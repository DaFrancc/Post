// Showing formatted text: messages with the markers dropped, and the
// composer with its markers dimmed in place. Both read the same parse
// (markup.dart).

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';

import '../theme.dart';
import 'markup.dart';

TextStyle _apply(TextStyle s, Mark m) => switch (m) {
  Mark.bold => s.copyWith(fontWeight: FontWeight.w700),
  Mark.italic => s.copyWith(fontStyle: FontStyle.italic),
  Mark.underline => s.copyWith(decoration: TextDecoration.combine([if (s.decoration != null) s.decoration!, TextDecoration.underline])),
  Mark.strike => s.copyWith(decoration: TextDecoration.combine([if (s.decoration != null) s.decoration!, TextDecoration.lineThrough])),
  Mark.spoiler => s,
};

TextStyle _code(TextStyle s) =>
    s.copyWith(fontFamily: PType.mono, fontSize: (s.fontSize ?? 14) * 0.92, color: PColors.codeText, backgroundColor: PColors.codeInline);

TextStyle _mention(TextStyle s, {bool you = false}) => s.copyWith(
  fontWeight: FontWeight.w600,
  color: you ? PColors.mentionYouText : PColors.mentionText,
  backgroundColor: you ? PColors.mentionYou : PColors.mention,
);

/// How mentions in the text below show: by the name each person shows as,
/// yours picked out, and what a click on one does. Without one, a mention
/// shows as its `@username`.
class Mentions extends InheritedWidget {
  const Mentions({super.key, required this.nameOf, required this.you, this.onTap, required super.child});

  /// The name to show for a username.
  final String Function(String username) nameOf;

  /// The viewer's username.
  final String you;

  /// A mention clicked, and where (window coordinates).
  final void Function(String username, Offset at)? onTap;

  static Mentions? maybeOf(BuildContext context) => context.dependOnInheritedWidgetOfExactType<Mentions>();

  @override
  bool updateShouldNotify(Mentions old) => true;
}

// ---------------------------------------------------------------- messages

/// One stretch of a message's text as drawn, for arrival effects: where it
/// starts among all the characters drawn, what it says, and how it looks.
class RevealRun {
  const RevealRun(this.start, this.text, this.style, {this.hidden = false});

  final int start;
  final String text;
  final TextStyle style;

  /// A spoiler's hidden text: drawn as a bar, never as letters.
  final bool hidden;
}

/// A message's text, formatted. Spoilers are hidden until clicked.
class MarkupText extends StatefulWidget {
  const MarkupText(this.text, {super.key, required this.style, this.shownUpTo, this.runs});

  final String text;
  final TextStyle style;

  /// For arrival effects: characters from this one on (counting every
  /// character drawn, in order) are left transparent but in place, so the
  /// text keeps its exact layout while an effect draws them its own way.
  /// Null draws everything.
  final int? shownUpTo;

  /// For arrival effects: filled with the runs drawn, in order, on every
  /// build.
  final List<RevealRun>? runs;

  @override
  State<MarkupText> createState() => _MarkupTextState();
}

class _MarkupTextState extends State<MarkupText> {
  late List<Block> _blocks = parseBlocks(widget.text);
  final _revealed = <int>{};
  final _recognizers = <TapGestureRecognizer>[];

  @override
  void didUpdateWidget(MarkupText old) {
    super.didUpdateWidget(old);
    if (old.text != widget.text) {
      _blocks = parseBlocks(widget.text);
      _revealed.clear();
    }
  }

  @override
  void dispose() {
    _disposeRecognizers();
    super.dispose();
  }

  void _disposeRecognizers() {
    for (final r in _recognizers) {
      r.dispose();
    }
    _recognizers.clear();
  }

  @override
  Widget build(BuildContext context) {
    _disposeRecognizers();
    final s = widget.text;
    var spoiler = 0;
    // Every character drawn goes through here, in order, so arrival effects
    // can count them and find each one on screen.
    var drawn = 0;
    final runs = widget.runs?..clear();
    final upTo = widget.shownUpTo;
    final mentions = Mentions.maybeOf(context);
    // A span's tap handler covers its own text, not its children's, so
    // every span made for clickable text carries it.
    InlineSpan run(String text, TextStyle st, {GestureRecognizer? tap}) {
      runs?.add(RevealRun(drawn, text, st));
      final cut = upTo == null ? text.length : (upTo - drawn).clamp(0, text.length);
      drawn += text.length;
      final cursor = tap == null ? MouseCursor.defer : SystemMouseCursors.click;
      if (cut == text.length) return TextSpan(text: text, style: st, recognizer: tap, mouseCursor: cursor);
      // Transparent, decorations and backgrounds too: same glyphs, same
      // layout, nothing visible.
      final clear = st.copyWith(color: Colors.transparent, backgroundColor: Colors.transparent, decorationColor: Colors.transparent);
      if (cut == 0) return TextSpan(text: text, style: clear);
      return TextSpan(
        children: [
          TextSpan(text: text.substring(0, cut), style: st, recognizer: tap, mouseCursor: cursor),
          TextSpan(text: text.substring(cut), style: clear),
        ],
      );
    }

    InlineSpan mention(Mention n, TextStyle st) {
      final u = n.username(s);
      if (mentions == null) return run('@$u', _mention(st));
      final onTap = mentions.onTap;
      final tap = onTap == null ? null : (TapGestureRecognizer()..onTapUp = (d) => onTap(u, d.globalPosition));
      if (tap != null) _recognizers.add(tap);
      return run('@${mentions.nameOf(u)}', _mention(st, you: u == mentions.you), tap: tap);
    }

    List<InlineSpan> inline(List<Inline> nodes, TextStyle st) => [
      for (final n in nodes)
        switch (n) {
          Plain() => run(s.substring(n.start, n.end), st),
          Escape() => run(s[n.end - 1], st),
          Code() => run(s.substring(n.contentStart, n.contentEnd), _code(st)),
          Mention() => mention(n, st),
          Styled(mark: Mark.spoiler) => _spoiler(spoiler++, n, st, inline, (text, style) {
            runs?.add(RevealRun(drawn, text, style, hidden: true));
            drawn += text.length;
          }),
          Styled() => TextSpan(children: inline(n.children, _apply(st, n.mark))),
        },
    ];

    final base = widget.style;
    if (_blocks case [Paragraph p]) return Text.rich(TextSpan(children: inline(p.inline, base)));
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        for (final b in _blocks)
          switch (b) {
            Paragraph() => Text.rich(TextSpan(children: inline(b.inline, base))),
            Quote() => _QuoteBox(
              child: Text.rich(
                TextSpan(
                  children: [
                    for (final (i, l) in b.lines.indexed) ...[if (i > 0) run('\n', base), ...inline(l.inline, base)],
                  ],
                ),
              ),
            ),
            CodeBlock() => _CodeBox(child: Text.rich(run(s.substring(b.codeStart, b.codeEnd), _CodeBox.textStyle(base)))),
          },
      ],
    );
  }

  /// Hidden: a solid bar the size of the text. Clicked: the text, on a faint
  /// tint so it stays visibly a spoiler.
  InlineSpan _spoiler(
    int index,
    Styled n,
    TextStyle st,
    List<InlineSpan> Function(List<Inline>, TextStyle) inline,
    void Function(String text, TextStyle style) counted,
  ) {
    final shown = _revealed.contains(index);
    if (shown) return TextSpan(children: inline(n.children, st.copyWith(backgroundColor: PColors.spoilerShown)));
    final r = TapGestureRecognizer()..onTap = () => setState(() => _revealed.add(index));
    _recognizers.add(r);
    // One span holding the text: a span's tap handler and label apply to its
    // own text only, not its children's.
    final hidden = st.copyWith(color: Colors.transparent, backgroundColor: PColors.spoiler, decoration: TextDecoration.none);
    final text = plainText(widget.text.substring(n.contentStart, n.contentEnd), revealSpoilers: true);
    counted(text, hidden);
    return TextSpan(text: text, style: hidden, recognizer: r, mouseCursor: SystemMouseCursors.click, semanticsLabel: 'Spoiler, hidden. Click to show.');
  }
}

class _QuoteBox extends StatelessWidget {
  const _QuoteBox({required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    return Container(
      margin: const EdgeInsets.symmetric(vertical: 2),
      padding: const EdgeInsets.only(left: 12),
      decoration: const BoxDecoration(
        border: Border(left: BorderSide(color: PColors.quoteBar, width: 3)),
      ),
      child: child,
    );
  }
}

class _CodeBox extends StatelessWidget {
  const _CodeBox({required this.child});

  /// The code, drawn.
  final Widget child;

  static TextStyle textStyle(TextStyle base) => base.copyWith(fontFamily: PType.mono, fontSize: 13, height: 1.5, color: PColors.codeText);

  @override
  Widget build(BuildContext context) {
    return Container(
      width: double.infinity,
      margin: const EdgeInsets.symmetric(vertical: 4),
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
      decoration: BoxDecoration(
        color: PColors.codeBg,
        border: Border.all(color: PColors.selected),
        borderRadius: BorderRadius.circular(10),
      ),
      child: SingleChildScrollView(scrollDirection: Axis.horizontal, child: child),
    );
  }
}

// ---------------------------------------------------------------- composer

/// The composer's text: formatting shown as you type, markers dimmed but
/// kept (the text is exactly what was typed), and misspelled words with a
/// wavy red underline.
class MarkupEditingController extends TextEditingController {
  MarkupEditingController({super.text});

  List<TextRange> _misspelled = const [];

  /// Words to underline as misspelled, as ranges of the current text.
  List<TextRange> get misspelled => _misspelled;
  set misspelled(List<TextRange> ranges) {
    _misspelled = ranges;
    notifyListeners();
  }

  @override
  TextSpan buildTextSpan({required BuildContext context, TextStyle? style, required bool withComposing}) {
    final base = style ?? const TextStyle();
    final t = text;
    var segs = editingSegments(t, base);
    // Misspellings only where the text still matches (a check may lag a
    // keystroke behind), and not inside code.
    for (final r in _misspelled) {
      if (r.end > t.length || r.start >= r.end) continue;
      segs = _decorate(segs, r.start, r.end, (s) {
        if (s.fontFamily == PType.mono) return s;
        return s.copyWith(
          decoration: TextDecoration.underline,
          decorationStyle: TextDecorationStyle.wavy,
          decorationColor: PColors.misspelled,
          decorationThickness: 1.6,
        );
      });
    }
    if (withComposing && value.isComposingRangeValid) {
      final c = value.composing;
      segs = _decorate(segs, c.start, c.end, (s) => s.copyWith(decoration: TextDecoration.underline, decorationStyle: TextDecorationStyle.solid));
    }
    return TextSpan(
      style: base,
      children: [for (final g in segs) TextSpan(text: t.substring(g.start, g.end), style: g.style)],
    );
  }
}

typedef Segment = ({int start, int end, TextStyle style});

/// Styled pieces covering all of `t`, in order, markers dimmed.
List<Segment> editingSegments(String t, TextStyle base) {
  final segs = <Segment>[];
  void add(int a, int b, TextStyle st) {
    if (b > a) segs.add((start: a, end: b, style: st));
  }

  TextStyle marker(TextStyle st) => st.copyWith(color: PColors.textFaint);
  void inline(List<Inline> nodes, TextStyle st) {
    for (final n in nodes) {
      switch (n) {
        case Plain():
          add(n.start, n.end, st);
        case Mention():
          add(n.start, n.end, _mention(st));
        case Escape():
          add(n.start, n.start + 1, marker(st));
          add(n.start + 1, n.end, st);
        case Code():
          final c = _code(st);
          add(n.start, n.contentStart, marker(c));
          add(n.contentStart, n.contentEnd, c);
          add(n.contentEnd, n.end, marker(c));
        case Styled():
          final s2 = n.mark == Mark.spoiler ? st.copyWith(backgroundColor: PColors.spoilerShown) : _apply(st, n.mark);
          add(n.start, n.contentStart, marker(s2));
          inline(n.children, s2);
          add(n.contentEnd, n.end, marker(s2));
      }
    }
  }

  for (final b in parseBlocks(t)) {
    switch (b) {
      case Paragraph():
        inline(b.inline, base);
      case Quote():
        for (final l in b.lines) {
          add(l.start, l.contentStart, marker(base).copyWith(fontWeight: FontWeight.w700));
          inline(l.inline, base);
        }
      case CodeBlock():
        final mono = _code(base).copyWith(backgroundColor: Colors.transparent);
        add(b.start, b.codeStart, marker(mono));
        add(b.codeStart, b.codeEnd, mono);
        add(b.codeEnd, b.end, marker(mono));
    }
  }
  // Fill what no block covers (the newlines between blocks) in the base style.
  final out = <Segment>[];
  var at = 0;
  for (final g in segs) {
    if (g.start > at) out.add((start: at, end: g.start, style: base));
    if (g.end > at) out.add((start: g.start < at ? at : g.start, end: g.end, style: g.style));
    if (g.end > at) at = g.end;
  }
  if (at < t.length) out.add((start: at, end: t.length, style: base));
  return out;
}

/// Restyle the part of `segs` between `a` and `b`, splitting where needed.
List<Segment> _decorate(List<Segment> segs, int a, int b, TextStyle Function(TextStyle) f) {
  final out = <Segment>[];
  for (final g in segs) {
    if (g.end <= a || g.start >= b) {
      out.add(g);
      continue;
    }
    if (g.start < a) out.add((start: g.start, end: a, style: g.style));
    final s = g.start < a ? a : g.start, e = g.end > b ? b : g.end;
    out.add((start: s, end: e, style: f(g.style)));
    if (g.end > b) out.add((start: b, end: g.end, style: g.style));
  }
  return out;
}
