// Searching messages: the box in the chat header, which suggests filters,
// people and past searches as you type, and the results, in the side
// panel's place, grouped by conversation and a page at a time. A result
// opens its message where it sits in the conversation.

import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:post/src/rust/api/client.dart';

import '../state/app_state.dart';
import '../state/search_spec.dart';
import '../theme.dart';
import '../ui/controls.dart';
import '../ui/format.dart';
import '../ui/markup_text.dart';
import 'home.dart' show Panel;

const _pageSize = 25;

// ---------------------------------------------------------------- the box

/// The search box: filters as chips, then words. While it has focus, a
/// list under it offers what fits what is being typed; Enter searches.
class SearchBox extends StatefulWidget {
  const SearchBox({super.key, required this.state, this.members = const []});

  final AppState state;

  /// The conversation's members, offered first for `from:`.
  final List<MemberInfo> members;

  @override
  State<SearchBox> createState() => _SearchBoxState();
}

/// One line in the suggestions: a heading, or something to choose.
class _Row {
  const _Row.heading(this.label, {this.trailing}) : act = null, child = null;
  const _Row(this.child, this.act) : label = null, trailing = null;

  final String? label;
  final Widget? trailing;
  final Widget? child;
  final VoidCallback? act;

  bool get choosable => act != null;
}

class _SearchBoxState extends State<SearchBox> {
  late final _text = TextEditingController(text: widget.state.searchText);
  final _portal = OverlayPortalController();
  final _link = LayerLink();

  /// Which suggestion Enter picks.
  int _picked = 0;

  /// Hidden after searching, until the box changes again.
  bool _quiet = false;

  AppState get _state => widget.state;
  FocusNode get _focus => _state.searchFocus;

  @override
  void initState() {
    super.initState();
    _focus.addListener(_focusChanged);
    _text.addListener(_typed);
  }

  @override
  void dispose() {
    _focus.removeListener(_focusChanged);
    _text.dispose();
    super.dispose();
  }

  void _focusChanged() {
    if (_focus.hasFocus) {
      _quiet = false;
      _portal.show();
    } else {
      _portal.hide();
    }
    setState(() {});
  }

  void _typed() {
    if (_text.text != _state.searchText) _state.editSearch(text: _text.text);
    setState(() {
      _quiet = false;
      _picked = 0;
    });
  }

  /// The word being typed: what suggestions are for.
  String get _tail {
    final t = _text.text;
    return t.endsWith(' ') ? '' : t.split(' ').last;
  }

  void _replaceTail(String with_) {
    final t = _text.text;
    final head = t.endsWith(' ') ? t : t.substring(0, t.length - _tail.length);
    _text.value = TextEditingValue(
      text: '$head$with_',
      selection: TextSelection.collapsed(offset: head.length + with_.length),
    );
  }

  /// Turn what is being typed into a chip (one per kind: a new one replaces
  /// the old).
  void _addFilter(SearchFilter f) {
    _state.editSearch(filters: [..._state.searchFilters.where((x) => x.kind != f.kind), f]);
    _replaceTail('');
    final rest = _text.text.trim();
    _text.value = TextEditingValue(
      text: rest,
      selection: TextSelection.collapsed(offset: rest.length),
    );
    _focus.requestFocus();
  }

  void _search() => _searchFor(SearchSpec(words: _text.text.trim(), filters: _state.searchFilters));

  void _searchFor(SearchSpec spec) {
    if (spec.isEmpty) return;
    _state.runSearch(spec);
    setState(() => _quiet = true);
  }

  void _clear() {
    _text.clear();
    _state.closeSearch();
  }

  List<String> _people(String like) {
    final q = like.toLowerCase();
    final seen = <String>{};
    final out = <String>[];
    for (final u in [...widget.members.map((m) => m.username), ..._state.knownPeople]) {
      if (!seen.add(u)) continue;
      final name = _state.personNow(u)?.shownName ?? u;
      if (q.isEmpty || u.toLowerCase().contains(q) || name.toLowerCase().contains(q)) out.add(u);
    }
    return out.take(8).toList();
  }

  List<_Row> _rows() {
    final tail = _tail;
    final colon = tail.indexOf(':');
    final kind = colon > 0 ? FilterKind.named(tail.substring(0, colon).toLowerCase()) : null;
    if (kind != null) {
      final value = tail.substring(colon + 1).replaceFirst('@', '');
      switch (kind) {
        case FilterKind.from || FilterKind.mentions:
          final people = _people(value);
          return [
            _Row.heading(kind == FilterKind.from ? 'From user' : 'Mentions user'),
            for (final u in people) _Row(_Person(state: _state, username: u), () => _addFilter(SearchFilter(kind, u))),
            if (people.isEmpty) const _Row.heading('No one by that name'),
          ];
        case FilterKind.has:
          return [
            const _Row.heading('Includes'),
            _Row(const _Choice(icon: Icons.link_rounded, title: 'A link'), () => _addFilter(const SearchFilter(FilterKind.has, 'link'))),
          ];
        case FilterKind.before || FilterKind.after || FilterKind.during:
          final now = DateTime.now();
          final today = DateTime(now.year, now.month, now.day);
          final typed = SearchFilter(kind, value);
          return [
            _Row.heading(kind.title),
            if (typed.valid) _Row(_Choice(icon: Icons.event_rounded, title: value), () => _addFilter(typed)),
            for (final (label, day) in [
              ('Today', today),
              ('Yesterday', today.subtract(const Duration(days: 1))),
              ('A week ago', today.subtract(const Duration(days: 7))),
            ])
              _Row(_Choice(icon: Icons.event_outlined, title: label, detail: dayText(day)), () => _addFilter(SearchFilter(kind, dayText(day)))),
            const _Row.heading('Or type a date: YYYY-MM-DD'),
          ];
      }
    }
    if (_text.text.trim().isEmpty) {
      return [
        const _Row.heading('Filters'),
        for (final k in FilterKind.values) _Row(_Choice(icon: _icons[k]!, title: k.title, detail: '${k.name}: ${k.hint}'), () => _replaceTail('${k.name}:')),
        if (_state.searchHistory.isNotEmpty) ...[
          _Row.heading(
            'History',
            trailing: PIconButton(icon: Icons.delete_outline_rounded, tooltip: 'Clear history', size: 26, iconSize: 15, onPressed: _state.clearSearchHistory),
          ),
          for (final h in _state.searchHistory.take(5))
            _Row(_Past(spec: h), () {
              _text.value = TextEditingValue(
                text: h.words,
                selection: TextSelection.collapsed(offset: h.words.length),
              );
              _searchFor(h);
            }),
        ],
      ];
    }
    final people = _people(tail);
    return [
      _Row(_Choice(icon: Icons.search_rounded, title: 'Search for ${_text.text.trim()}', strong: true), _search),
      if (tail.isNotEmpty && people.isNotEmpty) ...[
        const _Row.heading('From user'),
        for (final u in people.take(4)) _Row(_Person(state: _state, username: u), () => _addFilter(SearchFilter(FilterKind.from, u))),
      ],
    ];
  }

  static const _icons = {
    FilterKind.from: Icons.person_outline_rounded,
    FilterKind.mentions: Icons.alternate_email_rounded,
    FilterKind.has: Icons.link_rounded,
    FilterKind.before: Icons.event_outlined,
    FilterKind.after: Icons.event_outlined,
    FilterKind.during: Icons.today_outlined,
  };

  KeyEventResult _key(FocusNode _, KeyEvent e) {
    if (e is! KeyDownEvent && e is! KeyRepeatEvent) return KeyEventResult.ignored;
    final rows = _rows();
    final choosable = [
      for (final (i, r) in rows.indexed)
        if (r.choosable) i,
    ];
    switch (e.logicalKey) {
      case LogicalKeyboardKey.arrowDown || LogicalKeyboardKey.arrowUp when choosable.isNotEmpty && !_quiet:
        final down = e.logicalKey == LogicalKeyboardKey.arrowDown;
        setState(() => _picked = (_picked + (down ? 1 : -1)) % choosable.length);
        return KeyEventResult.handled;
      case LogicalKeyboardKey.enter || LogicalKeyboardKey.numpadEnter:
        if (!_quiet && choosable.isNotEmpty) {
          rows[choosable[_picked.clamp(0, choosable.length - 1)]].act!();
        } else {
          _search();
        }
        return KeyEventResult.handled;
      case LogicalKeyboardKey.backspace when _text.text.isEmpty && _state.searchFilters.isNotEmpty:
        _state.editSearch(filters: _state.searchFilters.sublist(0, _state.searchFilters.length - 1));
        return KeyEventResult.handled;
      case LogicalKeyboardKey.escape:
        if (_text.text.isNotEmpty || _state.searchFilters.isNotEmpty || _state.searchShown != null) {
          _clear();
        } else {
          _focus.unfocus();
        }
        return KeyEventResult.handled;
      default:
        return KeyEventResult.ignored;
    }
  }

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: _state,
      builder: (context, _) {
        final filters = _state.searchFilters;
        final active = _focus.hasFocus || _text.text.isNotEmpty || filters.isNotEmpty;
        // The suggestions hang from a point at the box's bottom-right corner,
        // not from the box: the box widens as it opens, and a follower
        // anchored to the box's size kept the size it first saw.
        return OverlayPortal(
          controller: _portal,
          overlayChildBuilder: _dropdown,
          child: Stack(
            clipBehavior: Clip.none,
            children: [
              _boxBody(filters, active),
              Positioned(
                right: 0,
                bottom: 0,
                child: CompositedTransformTarget(link: _link, child: const SizedBox.shrink()),
              ),
            ],
          ),
        );
      },
    );
  }

  /// The box grows downward rather than clipping: chips wrap onto new
  /// lines, and the words follow the last chip while they fit, then take a
  /// line of their own and wrap within it (up to four lines; Enter still
  /// searches, as in the composer).
  Widget _boxBody(List<SearchFilter> filters, bool active) {
    return AnimatedContainer(
      duration: PMotion.normal,
      curve: PMotion.curve,
      width: active ? 280 : 200,
      constraints: const BoxConstraints(minHeight: 36),
      decoration: BoxDecoration(
        color: PColors.sunken,
        border: Border.all(color: _focus.hasFocus ? PColors.focusEdge : PColors.border),
        borderRadius: BorderRadius.circular(11),
      ),
      child: Focus(
        onKeyEvent: _key,
        child: Row(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            const SizedBox(width: 6),
            Expanded(
              child: Padding(
                padding: EdgeInsets.symmetric(vertical: filters.isEmpty ? 0 : 6),
                child: LayoutBuilder(
                  builder: (context, box) => Wrap(
                    spacing: 4,
                    runSpacing: 4,
                    crossAxisAlignment: WrapCrossAlignment.center,
                    children: [
                      for (final f in filters) _Chip(filter: f),
                      // As wide as its words (at least a few letters), up to
                      // the whole line; with no chips, the whole line.
                      ConstrainedBox(
                        constraints: BoxConstraints(minWidth: filters.isEmpty ? box.maxWidth : 60, maxWidth: box.maxWidth),
                        child: IntrinsicWidth(child: _field(filters)),
                      ),
                    ],
                  ),
                ),
              ),
            ),
            Padding(
              padding: const EdgeInsets.only(top: 4),
              child: _text.text.isNotEmpty || filters.isNotEmpty || _state.searchShown != null
                  ? PIconButton(icon: Icons.close_rounded, tooltip: 'Clear (Esc)', size: 28, iconSize: 15, onPressed: _clear)
                  : const SizedBox(width: 28, height: 28, child: Icon(Icons.search_rounded, size: 17, color: PColors.textFaint)),
            ),
            const SizedBox(width: 4),
          ],
        ),
      ),
    );
  }

  Widget _field(List<SearchFilter> filters) {
    // Desktop's compact density would quietly take padding off the field,
    // leaving it shorter than the box and sitting at its top.
    return Theme(
      data: Theme.of(context).copyWith(visualDensity: VisualDensity.standard),
      child: _fieldItself(filters),
    );
  }

  Widget _fieldItself(List<SearchFilter> filters) {
    return TextField(
      controller: _text,
      focusNode: _focus,
      minLines: 1,
      maxLines: 4,
      // One line of words, wrapped to fit: a pasted line break is a space.
      inputFormatters: [FilteringTextInputFormatter.deny(RegExp(r'[\r\n]+'), replacementString: ' ')],
      style: const TextStyle(fontSize: 13.5, height: 18 / 13.5, color: PColors.text),
      decoration: InputDecoration(
        hintText: filters.isEmpty ? 'Search (Ctrl+F)' : null,
        hintStyle: const TextStyle(fontSize: 13.5, height: 18 / 13.5, color: PColors.textFaint),
        isDense: true,
        filled: false,
        // One line is 18 + 2×8: with the 1px border, the box's 36.
        contentPadding: EdgeInsets.symmetric(vertical: filters.isEmpty ? 8 : 3),
        border: InputBorder.none,
        enabledBorder: InputBorder.none,
        focusedBorder: InputBorder.none,
      ),
    );
  }

  Widget _dropdown(BuildContext context) {
    final rows = _quiet ? const <_Row>[] : _rows();
    if (rows.isEmpty) return const SizedBox.shrink();
    final choosable = [
      for (final (i, r) in rows.indexed)
        if (r.choosable) i,
    ];
    final picked = choosable.isEmpty ? -1 : choosable[_picked.clamp(0, choosable.length - 1)];
    return CompositedTransformFollower(
      link: _link,
      targetAnchor: Alignment.topLeft,
      followerAnchor: Alignment.topRight,
      offset: const Offset(0, 6),
      child: Align(
        alignment: Alignment.topRight,
        // Clicks here count as clicks in the box: it keeps focus.
        child: TextFieldTapRegion(
          child: TweenAnimationBuilder<double>(
            tween: Tween(begin: 0, end: 1),
            duration: PMotion.normal,
            curve: pMenuAnimation.curve!,
            builder: (context, v, child) => Opacity(
              opacity: v,
              child: Transform.translate(offset: Offset(0, -6 * (1 - v)), child: child),
            ),
            child: Container(
              width: 340,
              padding: const EdgeInsets.all(6),
              decoration: BoxDecoration(
                color: PColors.panelRaised,
                border: Border.all(color: PColors.borderStrong),
                borderRadius: BorderRadius.circular(14),
                boxShadow: const [BoxShadow(color: Color(0x80000000), blurRadius: 24, offset: Offset(0, 8))],
              ),
              child: Material(
                type: MaterialType.transparency,
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    for (final (i, r) in rows.indexed)
                      if (r.choosable)
                        _Choosable(highlighted: i == picked, onTap: r.act!, child: r.child!)
                      else
                        Padding(
                          padding: EdgeInsets.fromLTRB(10, i == 0 ? 4 : 10, 4, 4),
                          child: Row(
                            children: [
                              Expanded(child: Text(r.label!, style: PType.label)),
                              ?r.trailing,
                            ],
                          ),
                        ),
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

class _Chip extends StatelessWidget {
  const _Chip({required this.filter});

  final SearchFilter filter;

  @override
  Widget build(BuildContext context) {
    return Container(
      constraints: const BoxConstraints(maxWidth: 150),
      padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 3),
      decoration: BoxDecoration(color: PColors.selected, borderRadius: BorderRadius.circular(7)),
      child: Text.rich(
        TextSpan(
          children: [
            TextSpan(
              text: '${filter.kind.name}: ',
              style: const TextStyle(fontWeight: FontWeight.w700),
            ),
            TextSpan(text: filter.value),
          ],
        ),
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: const TextStyle(fontSize: 12.5, color: PColors.text),
      ),
    );
  }
}

class _Choosable extends StatefulWidget {
  const _Choosable({required this.highlighted, required this.onTap, required this.child});

  final bool highlighted;
  final VoidCallback onTap;
  final Widget child;

  @override
  State<_Choosable> createState() => _ChoosableState();
}

class _ChoosableState extends State<_Choosable> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: AnimatedContainer(
          duration: PMotion.fast,
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 7),
          decoration: BoxDecoration(
            color: widget.highlighted ? PColors.active : (_hover ? PColors.hoverStrong : Colors.transparent),
            borderRadius: BorderRadius.circular(9),
          ),
          child: widget.child,
        ),
      ),
    );
  }
}

class _Choice extends StatelessWidget {
  const _Choice({required this.icon, required this.title, this.detail, this.strong = false});

  final IconData icon;
  final String title;
  final String? detail;
  final bool strong;

  @override
  Widget build(BuildContext context) {
    return Row(
      children: [
        Icon(icon, size: 18, color: PColors.textMuted),
        const SizedBox(width: 10),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                title,
                maxLines: 1,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 13.5, fontWeight: strong ? FontWeight.w600 : FontWeight.w500, color: PColors.text),
              ),
              if (detail != null) Text(detail!, style: const TextStyle(fontSize: 12, color: PColors.textFaint)),
            ],
          ),
        ),
      ],
    );
  }
}

class _Person extends StatelessWidget {
  const _Person({required this.state, required this.username});

  final AppState state;
  final String username;

  @override
  Widget build(BuildContext context) {
    final name = state.personNow(username)?.shownName ?? username;
    return Row(
      children: [
        PAvatar(name: name, colorKey: username, size: 24, image: state.avatarOf(username)),
        const SizedBox(width: 10),
        Flexible(
          child: Text(name, maxLines: 1, overflow: TextOverflow.ellipsis, style: PType.name.copyWith(fontSize: 13.5)),
        ),
        const SizedBox(width: 6),
        Text(username, style: const TextStyle(fontSize: 12.5, color: PColors.textFaint)),
      ],
    );
  }
}

/// A past search: its filters as chips, then its words.
class _Past extends StatelessWidget {
  const _Past({required this.spec});

  final SearchSpec spec;

  @override
  Widget build(BuildContext context) {
    return Row(
      children: [
        const Icon(Icons.history_rounded, size: 18, color: PColors.textMuted),
        const SizedBox(width: 10),
        for (final f in spec.filters) ...[Flexible(flex: 0, child: _Chip(filter: f)), const SizedBox(width: 4)],
        if (spec.words.isNotEmpty)
          Flexible(
            child: Text(
              spec.words,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: const TextStyle(fontSize: 13.5, fontWeight: FontWeight.w600, color: PColors.text),
            ),
          ),
      ],
    );
  }
}

// ---------------------------------------------------------------- results

class SearchPanel extends StatefulWidget {
  const SearchPanel({super.key, required this.state});

  final AppState state;

  @override
  State<SearchPanel> createState() => _SearchPanelState();
}

class _SearchPanelState extends State<SearchPanel> {
  SearchFound? _found;
  String? _error;
  int _run = 0;
  final _scroll = ScrollController();

  /// What the results are for, to search again only when it changes.
  Object? _for;

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
    _scroll.dispose();
    super.dispose();
  }

  void _changed() {
    final spec = _state.searchShown;
    if (spec == null) return;
    final key = (spec, _state.searchAll, _state.searchOldest, _state.searchPage, _state.searchAll ? null : _state.selected);
    if (key == _for) return;
    _for = key;
    unawaited(_search(spec));
  }

  Future<void> _search(SearchSpec spec) async {
    final run = ++_run;
    try {
      final found = await _state.client.search(
        query: spec.options(
          conversation: _state.searchAll ? null : _state.selected,
          oldestFirst: _state.searchOldest,
          page: _state.searchPage,
          pageSize: _pageSize,
        ),
      );
      if (!mounted || run != _run) return;
      setState(() {
        _found = found;
        _error = null;
      });
      if (_scroll.hasClients) _scroll.jumpTo(0);
    } catch (e) {
      if (mounted && run == _run) setState(() => _error = errorText(e));
    }
  }

  static String _count(int n) {
    final s = '$n';
    final out = StringBuffer();
    for (var i = 0; i < s.length; i++) {
      if (i > 0 && (s.length - i) % 3 == 0) out.write(',');
      out.write(s[i]);
    }
    return out.toString();
  }

  Future<void> _menu(BuildContext anchor, List<(String, bool, VoidCallback)> choices) async {
    final box = anchor.findRenderObject()! as RenderBox;
    final at = box.localToGlobal(Offset(0, box.size.height + 4));
    final picked = await showContextMenu<int>(anchor, at, [
      for (final (i, (label, on, _)) in choices.indexed) PMenuItem(value: i, label: label, icon: on ? Icons.check_rounded : null),
    ]);
    if (picked != null) choices[picked].$3();
  }

  @override
  Widget build(BuildContext context) {
    final found = _found;
    final pages = found == null ? 0 : (found.total / _pageSize).ceil();
    // Mentions show by name; a click anywhere on a result opens it, so they
    // take no clicks of their own.
    return Mentions(
      nameOf: widget.state.knownName,
      you: widget.state.me.username,
      child: Panel(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Container(
              constraints: const BoxConstraints(minHeight: PShape.header),
              padding: const EdgeInsets.fromLTRB(16, 8, 8, 8),
              decoration: const BoxDecoration(
                border: Border(bottom: BorderSide(color: PColors.border)),
              ),
              // The count on its own line, so it never wraps; where and in what
              // order underneath.
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Row(
                    children: [
                      Expanded(
                        child: Text(
                          found == null ? 'Searching…' : '${_count(found.total)} ${found.total == 1 ? 'result' : 'results'}',
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: PType.heading,
                        ),
                      ),
                      PIconButton(icon: Icons.close_rounded, tooltip: 'Close search (Esc)', onPressed: _state.closeSearch),
                    ],
                  ),
                  Row(
                    children: [
                      Builder(
                        builder: (anchor) => _HeaderButton(
                          icon: Icons.filter_alt_outlined,
                          label: _state.searchAll ? 'Everywhere' : 'Here',
                          tooltip: 'Where to search',
                          onTap: () => _menu(anchor, [
                            ('This conversation', !_state.searchAll, () => _state.setSearchView(all: false)),
                            ('Every conversation', _state.searchAll, () => _state.setSearchView(all: true)),
                          ]),
                        ),
                      ),
                      const SizedBox(width: 4),
                      Builder(
                        builder: (anchor) => _HeaderButton(
                          icon: Icons.swap_vert_rounded,
                          label: _state.searchOldest ? 'Oldest' : 'Newest',
                          tooltip: 'Sort',
                          onTap: () => _menu(anchor, [
                            ('Newest first', !_state.searchOldest, () => _state.setSearchView(oldest: false)),
                            ('Oldest first', _state.searchOldest, () => _state.setSearchView(oldest: true)),
                          ]),
                        ),
                      ),
                    ],
                  ),
                ],
              ),
            ),
            Expanded(
              child: _error != null
                  ? Padding(padding: const EdgeInsets.all(16), child: PErrorBox(_error!))
                  : found == null
                  ? const Center(child: PSpinner())
                  : found.messages.isEmpty
                  ? const Padding(
                      padding: EdgeInsets.all(24),
                      child: Text(
                        'Nothing found.\nTry fewer words, another filter, or every conversation.',
                        textAlign: TextAlign.center,
                        style: TextStyle(fontSize: 13, height: 1.5, color: PColors.textFaint),
                      ),
                    )
                  : ListView(
                      controller: _scroll,
                      padding: const EdgeInsets.fromLTRB(10, 4, 10, 12),
                      children: [
                        for (final (i, m) in found.messages.indexed) ...[
                          // A heading where the conversation changes.
                          if (i == 0 || found.messages[i - 1].conversation != m.conversation) _Where(state: _state, conversation: m.conversation),
                          _Card(m: m, image: _state.avatarOf(m.senderUsername), onTap: () => _state.jumpTo(m.conversation, m.id)),
                        ],
                      ],
                    ),
            ),
            if (pages > 1)
              Container(
                padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 8),
                decoration: const BoxDecoration(
                  border: Border(top: BorderSide(color: PColors.border)),
                ),
                child: _Pages(
                  page: _state.searchPage,
                  pages: pages,
                  onPage: (p) => _state.setSearchView(page: p),
                ),
              ),
          ],
        ),
      ),
    );
  }
}

class _HeaderButton extends StatelessWidget {
  const _HeaderButton({required this.icon, required this.label, required this.tooltip, required this.onTap});

  final IconData icon;
  final String label;
  final String tooltip;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return Tooltip(
      message: tooltip,
      waitDuration: const Duration(milliseconds: 500),
      // A compact secondary button, tactile like the rest.
      child: OutlinedButton.icon(
        onPressed: onTap,
        icon: Icon(icon, size: 16),
        label: Text(label),
        style: OutlinedButton.styleFrom(
          padding: const EdgeInsets.symmetric(horizontal: 10),
          minimumSize: const Size(0, 32),
          textStyle: const TextStyle(fontSize: 12.5, fontWeight: FontWeight.w600),
        ),
      ),
    );
  }
}

/// The conversation a run of results is from.
class _Where extends StatelessWidget {
  const _Where({required this.state, required this.conversation});

  final AppState state;
  final String conversation;

  @override
  Widget build(BuildContext context) {
    final c = state.conversationById(conversation);
    return Padding(
      padding: const EdgeInsets.fromLTRB(6, 10, 6, 6),
      child: Row(
        children: [
          Icon(c?.isGroup ?? false ? Icons.tag_rounded : Icons.alternate_email_rounded, size: 15, color: PColors.textFaint),
          const SizedBox(width: 6),
          Expanded(
            child: Text(
              c?.title ?? 'A conversation',
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: const TextStyle(fontSize: 13, fontWeight: FontWeight.w700, color: PColors.textMuted),
            ),
          ),
        ],
      ),
    );
  }
}

/// One result, as the message looks in the conversation.
class _Card extends StatefulWidget {
  const _Card({required this.m, required this.image, required this.onTap});

  final MessageInfo m;

  /// The sender's picture, if it is here.
  final PictureRef? image;
  final VoidCallback onTap;

  @override
  State<_Card> createState() => _CardState();
}

class _CardState extends State<_Card> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final m = widget.m;
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: widget.onTap,
        child: AnimatedContainer(
          duration: PMotion.fast,
          margin: const EdgeInsets.only(bottom: 8),
          padding: const EdgeInsets.fromLTRB(12, 10, 12, 11),
          decoration: BoxDecoration(
            color: _hover ? PColors.hover : PColors.panelRaised,
            border: Border.all(color: _hover ? PColors.borderStrong : PColors.border),
            borderRadius: BorderRadius.circular(12),
          ),
          child: Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              PAvatar(name: m.sender, colorKey: m.senderUsername, size: 34, image: widget.image),
              const SizedBox(width: 10),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Row(
                      crossAxisAlignment: CrossAxisAlignment.baseline,
                      textBaseline: TextBaseline.alphabetic,
                      children: [
                        Flexible(
                          child: Text(m.sender, maxLines: 1, overflow: TextOverflow.ellipsis, style: PType.name),
                        ),
                        const SizedBox(width: 8),
                        Text(messageTime(m.sentAtMs), style: const TextStyle(fontSize: 11.5, color: PColors.textFaint)),
                      ],
                    ),
                    const SizedBox(height: 2),
                    // Not selectable: a click opens the message.
                    IgnorePointer(child: MarkupText(m.body ?? '', style: PType.text.copyWith(fontSize: 13.5))),
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

/// Previous, page numbers (the first, the last and those around this one,
/// with a gap that takes a typed number), Next.
class _Pages extends StatefulWidget {
  const _Pages({required this.page, required this.pages, required this.onPage});

  final int page;
  final int pages;
  final ValueChanged<int> onPage;

  @override
  State<_Pages> createState() => _PagesState();
}

class _PagesState extends State<_Pages> {
  bool _typing = false;
  final _typed = TextEditingController();

  @override
  void dispose() {
    _typed.dispose();
    super.dispose();
  }

  void _go(int p) {
    setState(() => _typing = false);
    widget.onPage(p.clamp(0, widget.pages - 1));
  }

  @override
  Widget build(BuildContext context) {
    final p = widget.page, n = widget.pages;
    final shown = <int>{0, n - 1, for (var i = math.max(0, p - 1); i <= math.min(n - 1, p + 1); i++) i}.toList()..sort();
    final items = <Widget>[];
    for (final (i, page) in shown.indexed) {
      if (i > 0 && page - shown[i - 1] > 1) {
        items.add(
          _typing
              ? SizedBox(
                  width: 52,
                  height: 32,
                  child: TextField(
                    controller: _typed,
                    autofocus: true,
                    textAlign: TextAlign.center,
                    keyboardType: TextInputType.number,
                    inputFormatters: [FilteringTextInputFormatter.digitsOnly],
                    style: const TextStyle(fontSize: 13),
                    decoration: InputDecoration(
                      isDense: true,
                      contentPadding: const EdgeInsets.symmetric(vertical: 8),
                      enabledBorder: OutlineInputBorder(
                        borderRadius: BorderRadius.circular(9),
                        borderSide: const BorderSide(color: PColors.accent),
                      ),
                      focusedBorder: OutlineInputBorder(
                        borderRadius: BorderRadius.circular(9),
                        borderSide: const BorderSide(color: PColors.accent),
                      ),
                    ),
                    onSubmitted: (s) => _go((int.tryParse(s) ?? p + 1) - 1),
                    onTapOutside: (_) => setState(() => _typing = false),
                  ),
                )
              : _PageButton(
                  label: '…',
                  tooltip: 'Go to a page',
                  onTap: () => setState(() {
                    _typed.clear();
                    _typing = true;
                  }),
                ),
        );
      }
      items.add(_PageButton(label: '${page + 1}', on: page == p, onTap: () => _go(page)));
    }
    return Row(
      children: [
        TextButton.icon(onPressed: p > 0 ? () => _go(p - 1) : null, icon: const Icon(Icons.chevron_left_rounded, size: 18), label: const Text('Previous')),
        Expanded(
          child: Row(mainAxisAlignment: MainAxisAlignment.center, children: items),
        ),
        Directionality(
          textDirection: TextDirection.rtl,
          child: TextButton.icon(
            onPressed: p < n - 1 ? () => _go(p + 1) : null,
            icon: const Icon(Icons.chevron_left_rounded, size: 18, textDirection: TextDirection.rtl),
            label: const Text('Next'),
          ),
        ),
      ],
    );
  }
}

class _PageButton extends StatelessWidget {
  const _PageButton({required this.label, required this.onTap, this.on = false, this.tooltip});

  final String label;
  final VoidCallback onTap;
  final bool on;
  final String? tooltip;

  @override
  Widget build(BuildContext context) {
    final button = MouseRegion(
      cursor: SystemMouseCursors.click,
      child: GestureDetector(
        onTap: onTap,
        child: AnimatedContainer(
          duration: PMotion.fast,
          constraints: const BoxConstraints(minWidth: 30),
          height: 30,
          margin: const EdgeInsets.symmetric(horizontal: 2),
          padding: const EdgeInsets.symmetric(horizontal: 6),
          alignment: Alignment.center,
          decoration: BoxDecoration(color: on ? PColors.accent : Colors.transparent, borderRadius: BorderRadius.circular(15)),
          child: Text(
            label,
            style: TextStyle(fontSize: 13, fontWeight: FontWeight.w600, color: on ? PColors.onAccent : PColors.textMuted),
          ),
        ),
      ),
    );
    return tooltip == null ? button : Tooltip(message: tooltip!, child: button);
  }
}
