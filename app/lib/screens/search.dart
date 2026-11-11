// Search results, in the side panel's place while a search is open: in the
// conversation on screen or in all of them, newest first, the words found
// lit. `from:name` in the search keeps to one sender. A result opens its
// message where it sits in the conversation.

import 'dart:async';

import 'package:flutter/material.dart';
import 'package:post/src/rust/api/client.dart';

import '../state/app_state.dart';
import '../theme.dart';
import '../ui/controls.dart';
import '../ui/format.dart';
import '../ui/markup.dart';
import 'home.dart' show Panel;

/// A search as typed: its words, and a sender from `from:name` (an `@` is
/// allowed: `from:@name`).
({String words, String? from}) parseSearch(String typed) {
  String? from;
  final words = <String>[];
  for (final t in typed.split(RegExp(r'\s+')).where((t) => t.isNotEmpty)) {
    if (t.toLowerCase().startsWith('from:') && t.length > 5) {
      from = t.substring(5).replaceFirst('@', '').toLowerCase();
    } else {
      words.add(t);
    }
  }
  return (words: words.join(' '), from: from);
}

class SearchPanel extends StatefulWidget {
  const SearchPanel({super.key, required this.state});

  final AppState state;

  @override
  State<SearchPanel> createState() => _SearchPanelState();
}

class _SearchPanelState extends State<SearchPanel> {
  List<MessageInfo>? _results;
  String? _error;
  Timer? _soon;
  int _run = 0;

  /// What the results are for, to search again only when it changes.
  (String, bool, String?)? _for;

  AppState get _state => widget.state;

  @override
  void initState() {
    super.initState();
    _state.addListener(_changed);
    _changed();
  }

  @override
  void dispose() {
    _state.removeListener(_changed);
    _soon?.cancel();
    super.dispose();
  }

  void _changed() {
    final key = (_state.searchText, _state.searchAll, _state.searchAll ? null : _state.selected);
    if (key == _for) return;
    _for = key;
    _soon?.cancel();
    // A moment after typing stops, not on every key.
    _soon = Timer(const Duration(milliseconds: 180), () => unawaited(_search()));
  }

  Future<void> _search() async {
    final run = ++_run;
    final q = parseSearch(_state.searchText);
    try {
      final found = await _state.client.search(text: q.words, conversation: _state.searchAll ? null : _state.selected, from: q.from, limit: 100);
      if (mounted && run == _run) {
        setState(() {
          _results = found;
          _error = null;
        });
      }
    } catch (e) {
      if (mounted && run == _run) setState(() => _error = errorText(e));
    }
  }

  @override
  Widget build(BuildContext context) {
    final results = _results;
    final q = parseSearch(_state.searchText);
    final words = q.words.toLowerCase().split(' ').where((w) => w.isNotEmpty).toList();
    return Panel(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Container(
            constraints: const BoxConstraints(minHeight: PShape.header),
            padding: const EdgeInsets.fromLTRB(16, 8, 8, 8),
            decoration: const BoxDecoration(
              border: Border(bottom: BorderSide(color: PColors.border)),
            ),
            child: Row(
              children: [
                Expanded(
                  child: Text(
                    results == null ? 'Searching…' : '${results.length == 100 ? '100+' : results.length} ${results.length == 1 ? 'result' : 'results'}',
                    style: PType.heading,
                  ),
                ),
                PIconButton(icon: Icons.close_rounded, tooltip: 'Close search (Esc)', onPressed: _state.closeSearch),
              ],
            ),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(12, 10, 12, 4),
            child: Row(
              children: [
                _Scope(label: 'This conversation', on: !_state.searchAll, onTap: () => _state.setSearch(_state.searchText, all: false)),
                const SizedBox(width: 6),
                _Scope(label: 'Everywhere', on: _state.searchAll, onTap: () => _state.setSearch(_state.searchText, all: true)),
              ],
            ),
          ),
          if (q.from != null)
            Padding(
              padding: const EdgeInsets.fromLTRB(16, 4, 16, 0),
              child: Text('From @${q.from}', style: const TextStyle(fontSize: 12, color: PColors.textFaint)),
            ),
          Expanded(
            child: _error != null
                ? Padding(padding: const EdgeInsets.all(16), child: PErrorBox(_error!))
                : results == null
                ? const Center(child: PSpinner())
                : results.isEmpty
                ? const Padding(
                    padding: EdgeInsets.all(20),
                    child: Text(
                      'Nothing found. Try fewer words, or search everywhere.\nfrom:name keeps to one person.',
                      textAlign: TextAlign.center,
                      style: TextStyle(fontSize: 13, height: 1.5, color: PColors.textFaint),
                    ),
                  )
                : ListView.builder(
                    padding: const EdgeInsets.fromLTRB(8, 6, 8, 12),
                    itemCount: results.length,
                    itemBuilder: (context, i) => _Hit(
                      m: results[i],
                      words: words,
                      where: _state.searchAll ? _state.conversationById(results[i].conversation)?.title : null,
                      onTap: () => _state.jumpTo(results[i].conversation, results[i].id),
                    ),
                  ),
          ),
        ],
      ),
    );
  }
}

class _Scope extends StatelessWidget {
  const _Scope({required this.label, required this.on, required this.onTap});

  final String label;
  final bool on;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: onTap,
        child: AnimatedContainer(
          duration: PMotion.fast,
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
          decoration: BoxDecoration(
            color: on ? PColors.active : PColors.panelRaised,
            border: Border.all(color: on ? PColors.activeEdge : PColors.border),
            borderRadius: BorderRadius.circular(10),
          ),
          child: Text(
            label,
            style: TextStyle(fontSize: 12.5, fontWeight: FontWeight.w500, color: on ? PColors.accentText : PColors.textMuted),
          ),
        ),
      ),
    );
  }
}

/// One result: who, when, where (searching everywhere), and the text with
/// the words found lit.
class _Hit extends StatefulWidget {
  const _Hit({required this.m, required this.words, required this.where, required this.onTap});

  final MessageInfo m;
  final List<String> words;
  final String? where;
  final VoidCallback onTap;

  @override
  State<_Hit> createState() => _HitState();
}

class _HitState extends State<_Hit> {
  bool _hover = false;

  /// The text with every occurrence of the words picked out.
  List<TextSpan> _lit(String text) {
    final lower = text.toLowerCase();
    final marks = List<bool>.filled(text.length, false);
    for (final w in widget.words) {
      for (var i = lower.indexOf(w); i >= 0; i = lower.indexOf(w, i + 1)) {
        // Lower-casing can change length in rare scripts; skip what no
        // longer lines up rather than mark the wrong letters.
        if (lower.length != text.length) break;
        for (var j = i; j < i + w.length; j++) {
          marks[j] = true;
        }
      }
    }
    final spans = <TextSpan>[];
    var start = 0;
    for (var i = 1; i <= text.length; i++) {
      if (i == text.length || marks[i] != marks[start]) {
        spans.add(
          TextSpan(
            text: text.substring(start, i),
            style: marks[start] ? const TextStyle(color: PColors.textStrong, backgroundColor: Color(0x4D6E8BFF), fontWeight: FontWeight.w600) : null,
          ),
        );
        start = i;
      }
    }
    return spans;
  }

  @override
  Widget build(BuildContext context) {
    final m = widget.m;
    final text = plainText(m.body ?? '', revealSpoilers: false);
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: AnimatedContainer(
          duration: PMotion.fast,
          margin: const EdgeInsets.only(bottom: 4),
          padding: const EdgeInsets.fromLTRB(10, 8, 10, 9),
          decoration: BoxDecoration(color: _hover ? PColors.hover : Colors.transparent, borderRadius: BorderRadius.circular(12)),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              PAvatar(name: m.sender, colorKey: m.senderUsername, size: 28),
              const SizedBox(width: 10),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      children: [
                        Flexible(
                          child: Text(m.sender, maxLines: 1, overflow: TextOverflow.ellipsis, style: PType.name.copyWith(fontSize: 13)),
                        ),
                        const SizedBox(width: 6),
                        Text(messageTime(m.sentAtMs), style: const TextStyle(fontSize: 11, color: PColors.textFaint)),
                      ],
                    ),
                    if (widget.where != null)
                      Text(
                        'in ${widget.where}',
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(fontSize: 11.5, color: PColors.textFaint),
                      ),
                    const SizedBox(height: 2),
                    Text.rich(
                      TextSpan(children: _lit(text)),
                      maxLines: 4,
                      overflow: TextOverflow.ellipsis,
                      style: const TextStyle(fontSize: 13, height: 1.45, color: PColors.textBody),
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}
