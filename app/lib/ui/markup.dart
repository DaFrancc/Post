// Message formatting: a small Markdown-like syntax, the kind chat apps use.
// Messages are sent as the text the user typed; this reads the formatting
// out of it, for showing messages, for live styling in the composer, and for
// plain previews.
//
//   **bold**  *italic* or _italic_  __underline__  ~~strike~~
//   `code`  ```code block```  ||spoiler||  > quote  \* (a literal *)
//   @username (a mention)
//
// The parser keeps every node's place in the source, so the composer can
// style the text in place (markers dimmed, never hidden) and the message
// view can drop the markers.

/// An inline style.
enum Mark { bold, italic, underline, strike, spoiler }

// ---------------------------------------------------------------- inline

/// A run of inline content, with its range in the source.
sealed class Inline {
  const Inline(this.start, this.end);

  final int start;
  final int end;
}

/// Plain text.
class Plain extends Inline {
  const Plain(super.start, super.end);
}

/// A backslash and the character it makes literal.
class Escape extends Inline {
  const Escape(super.start, super.end);
}

/// Text between two markers, `open` characters each.
class Styled extends Inline {
  const Styled(super.start, super.end, this.mark, this.open, this.children);

  final Mark mark;
  final int open;
  final List<Inline> children;

  int get contentStart => start + open;
  int get contentEnd => end - open;
}

/// Inline code: literal text between runs of `ticks` backticks.
class Code extends Inline {
  const Code(super.start, super.end, this.ticks);

  final int ticks;

  int get contentStart => start + ticks;
  int get contentEnd => end - ticks;
}

/// `@username`: a mention (design section 7, "Mentions").
class Mention extends Inline {
  const Mention(super.start, super.end);

  /// Who it mentions, read from the source.
  String username(String s) => s.substring(start + 1, end);
}

/// Where the mention starting with the `@` at `i` ends, or -1 if there is
/// none. The same rule as the core's (`content::mentions`): a username (3
/// to 32 of a-z, 0-9 and `_`, starting with a letter) with no letter, digit
/// or `_` touching it on either side.
int _mentionEnd(String s, int i) {
  if (s[i] != '@' || _isNameChar(s, i - 1)) return -1;
  var k = i + 1;
  while (k < s.length && _isUsernameChar(s.codeUnitAt(k))) {
    k++;
  }
  final len = k - i - 1;
  if (len < 3 || len > 32 || !_isLower(s.codeUnitAt(i + 1)) || _isNameChar(s, k)) return -1;
  return k;
}

bool _isLower(int c) => c >= 0x61 && c <= 0x7a;
bool _isUsernameChar(int c) => _isLower(c) || (c >= 0x30 && c <= 0x39) || c == 0x5f;
bool _isNameChar(String s, int i) => _isWordChar(s, i) || (i >= 0 && i < s.length && s[i] == '_');

/// Markers, longest first so `**` is tried before `*`.
const _markers = <(String, Mark)>[('||', Mark.spoiler), ('**', Mark.bold), ('__', Mark.underline), ('~~', Mark.strike), ('*', Mark.italic), ('_', Mark.italic)];

bool _isSpace(String s, int i) => i < 0 || i >= s.length || s[i].trim().isEmpty;
bool _isWordChar(String s, int i) => i >= 0 && i < s.length && RegExp(r'[\p{L}\p{N}]', unicode: true).hasMatch(s[i]);
bool _isPunct(String c) => RegExp(r'''[!"#$%&'()*+,\-./:;<=>?@\[\\\]^_`{|}~]''').hasMatch(c);

/// The length of the run of `c` at `i`, stopping at `to`.
int _run(String s, int i, int to, String c) {
  var n = 0;
  while (i + n < to && s[i + n] == c) {
    n++;
  }
  return n;
}

/// Where the code span opening with a backtick run at `i` closes (the index
/// after its closing run), or -1.
int _codeEnd(String s, int i, int to) {
  final n = _run(s, i, to, '`');
  var k = i + n;
  while (k < to) {
    if (s[k] == '`') {
      final m = _run(s, k, to, '`');
      if (m == n) return k + m;
      k += m;
    } else {
      k++;
    }
  }
  return -1;
}

/// Where a closing `marker` for an opener ending at `from` starts, or -1.
int _findClose(String s, String marker, int from, int to) {
  final c = marker[0];
  final len = marker.length;
  var k = from;
  while (k < to) {
    final ch = s[k];
    if (ch == '\\' && k + 1 < to && _isPunct(s[k + 1])) {
      k += 2;
      continue;
    }
    if (ch == '`') {
      final e = _codeEnd(s, k, to);
      if (e > 0) {
        k = e;
        continue;
      }
    }
    // A mention is whole: a `_` in a username closes nothing.
    if (ch == '@') {
      final e = _mentionEnd(s, k);
      if (e > 0 && e <= to) {
        k = e;
        continue;
      }
    }
    if (ch == c) {
      final run = _run(s, k, to, c);
      // A run longer than the marker closes at its end: `***x***` is bold
      // around italic. A single marker skips a double one (`*a **b** c*`).
      // (Every two-character marker is a doubled character.)
      int? at;
      if (len == 1) {
        if (run == 1 || run >= 3) at = k + run - 1;
      } else if (run >= 2) {
        at = k + run - 2;
      }
      if (at != null && at > from && !_isSpace(s, at - 1)) {
        final after = at + len;
        final wordy = c == '_' && _isWordChar(s, after);
        if (!wordy) return at;
      }
      k += run;
      continue;
    }
    k++;
  }
  return -1;
}

/// The inline content of `s` between `from` and `to`.
List<Inline> parseInline(String s, [int from = 0, int? to]) {
  final end = to ?? s.length;
  final out = <Inline>[];
  var text = from;
  var i = from;
  void flush(int upTo) {
    if (upTo > text) out.add(Plain(text, upTo));
  }

  outer:
  while (i < end) {
    final ch = s[i];
    if (ch == '\\' && i + 1 < end && _isPunct(s[i + 1])) {
      flush(i);
      out.add(Escape(i, i + 2));
      i += 2;
      text = i;
      continue;
    }
    if (ch == '`') {
      final e = _codeEnd(s, i, end);
      final n = _run(s, i, end, '`');
      if (e > 0 && e - i > 2 * n) {
        flush(i);
        out.add(Code(i, e, n));
        i = e;
        text = i;
      } else {
        i += n;
      }
      continue;
    }
    if (ch == '@') {
      final e = _mentionEnd(s, i);
      if (e > 0 && e <= end) {
        flush(i);
        out.add(Mention(i, e));
        i = e;
        text = i;
        continue;
      }
    }
    for (final (marker, mark) in _markers) {
      if (!s.startsWith(marker, i) || i + marker.length > end) continue;
      final len = marker.length;
      // A single marker that is really part of a double one waits for it.
      if (len == 1 && i + 1 < end && s[i + 1] == marker) continue;
      if (_isSpace(s, i + len)) continue;
      if (marker[0] == '_' && _isWordChar(s, i - 1)) continue;
      final close = _findClose(s, marker, i + len, end);
      if (close < 0) continue;
      flush(i);
      out.add(Styled(i, close + len, mark, len, parseInline(s, i + len, close)));
      i = close + len;
      text = i;
      continue outer;
    }
    i++;
  }
  flush(end);
  return out;
}

// ---------------------------------------------------------------- blocks

sealed class Block {
  const Block(this.start, this.end);

  final int start;
  final int end;
}

/// Lines of ordinary text.
class Paragraph extends Block {
  const Paragraph(super.start, super.end, this.inline);

  final List<Inline> inline;
}

/// One line of a quote: its `> ` marker, then content.
class QuoteLine {
  const QuoteLine(this.start, this.contentStart, this.end, this.inline);

  final int start;
  final int contentStart;
  final int end;
  final List<Inline> inline;
}

/// Consecutive `> ` lines.
class Quote extends Block {
  Quote(this.lines) : super(lines.first.start, lines.last.end);

  final List<QuoteLine> lines;
}

/// A fenced code block: ```lang, lines of code, ```.
class CodeBlock extends Block {
  const CodeBlock(super.start, super.end, this.language, this.codeStart, this.codeEnd, this.closeStart);

  final String language;
  final int codeStart;
  final int codeEnd;
  final int closeStart;
}

typedef _Line = ({int start, int end});

List<_Line> _lines(String s) {
  final out = <_Line>[];
  var start = 0;
  for (var i = 0; i <= s.length; i++) {
    if (i == s.length || s[i] == '\n') {
      out.add((start: start, end: i));
      start = i + 1;
    }
  }
  return out;
}

bool _isFence(String s, _Line l) => s.startsWith('```', l.start) && l.end - l.start >= 3;

bool _isQuote(String s, _Line l) => s.startsWith('> ', l.start) || (l.end - l.start == 1 && s[l.start] == '>');

/// The blocks of a message.
List<Block> parseBlocks(String s) {
  final lines = _lines(s);
  final out = <Block>[];
  var i = 0;
  while (i < lines.length) {
    final l = lines[i];
    // A fence opens a block when its own line does not close it too
    // (```x``` on one line is inline code) and a later line closes it.
    if (_isFence(s, l) && !s.substring(l.start + 3, l.end).contains('```')) {
      final close = lines.indexWhere((c) => _isFence(s, c), i + 1);
      if (close > 0) {
        final c = lines[close];
        final codeStart = close == i + 1 ? c.start : lines[i + 1].start;
        final codeEnd = close == i + 1 ? c.start : c.start - 1;
        out.add(CodeBlock(l.start, c.end, s.substring(l.start + 3, l.end).trim(), codeStart, codeEnd, c.start));
        i = close + 1;
        continue;
      }
    }
    if (_isQuote(s, l)) {
      final quoted = <QuoteLine>[];
      while (i < lines.length && _isQuote(s, lines[i])) {
        final q = lines[i];
        final content = (q.start + 2).clamp(q.start, q.end);
        quoted.add(QuoteLine(q.start, content, q.end, parseInline(s, content, q.end)));
        i++;
      }
      out.add(Quote(quoted));
      continue;
    }
    final first = i;
    while (i < lines.length && !_isQuote(s, lines[i]) && !(_isFence(s, lines[i]) && i != first && _opensBlock(s, lines, i))) {
      i++;
    }
    final start = lines[first].start, end = lines[i - 1].end;
    out.add(Paragraph(start, end, parseInline(s, start, end)));
  }
  return out;
}

bool _opensBlock(String s, List<_Line> lines, int i) {
  final l = lines[i];
  return !s.substring(l.start + 3, l.end).contains('```') && lines.indexWhere((c) => _isFence(s, c), i + 1) > 0;
}

// ---------------------------------------------------------------- editing

/// Text and a selection, as the formatting commands take and give them.
typedef Edit = ({String text, int start, int end});

/// The marker for each style the composer can toggle.
const markerFor = {Mark.bold: '**', Mark.italic: '*', Mark.underline: '__', Mark.strike: '~~', Mark.spoiler: '||'};

/// Wrap the selection in `marker`, or unwrap it if it is already wrapped
/// (just outside or just inside the selection). With nothing selected, the
/// markers go in with the cursor between them.
Edit toggleWrap(Edit e, String marker) {
  final t = e.text, s = e.start, end = e.end, m = marker.length;
  if (s == end) {
    return (text: t.replaceRange(s, s, marker * 2), start: s + m, end: s + m);
  }
  // Wrapped just outside. For `*`, the `*` of a `**` does not count.
  bool outside() {
    if (s < m || end + m > t.length) return false;
    if (t.substring(s - m, s) != marker || t.substring(end, end + m) != marker) return false;
    if (m == 1 && (s - 2 >= 0 && t[s - 2] == marker || end + 1 < t.length && t[end + 1] == marker)) return false;
    return true;
  }

  if (outside()) {
    final out = t.replaceRange(end, end + m, '').replaceRange(s - m, s, '');
    return (text: out, start: s - m, end: end - m);
  }
  final inner = t.substring(s, end);
  if (inner.length >= 2 * m && inner.startsWith(marker) && inner.endsWith(marker) && !(m == 1 && inner.startsWith(marker * 2))) {
    final unwrapped = inner.substring(m, inner.length - m);
    return (text: t.replaceRange(s, end, unwrapped), start: s, end: s + unwrapped.length);
  }
  return (text: t.replaceRange(s, end, '$marker$inner$marker'), start: s + m, end: end + m);
}

/// Quote the lines the selection touches, or unquote them if all are quoted.
Edit toggleQuote(Edit e) {
  final t = e.text;
  final lineStart = e.start == 0 ? 0 : t.lastIndexOf('\n', e.start - 1) + 1;
  var lineEnd = t.indexOf('\n', e.end == e.start ? e.end : (e.end - 1).clamp(0, t.length));
  if (lineEnd < 0) lineEnd = t.length;
  final lines = t.substring(lineStart, lineEnd).split('\n');
  final quoted = lines.every((l) => l.startsWith('> ') || l == '>');
  final changed = lines.map((l) => quoted ? (l == '>' ? '' : l.substring(2)) : '> $l').join('\n');
  final delta = changed.length - (lineEnd - lineStart);
  final firstDelta = quoted ? -(lines.first == '>' ? 1 : 2) : 2;
  final start = (e.start + firstDelta).clamp(lineStart, lineStart + changed.length);
  return (text: t.replaceRange(lineStart, lineEnd, changed), start: start, end: (e.end + delta).clamp(start, lineStart + changed.length));
}

/// Code: a block (```) for a selection over several lines, inline otherwise.
Edit toggleCode(Edit e) {
  final inner = e.text.substring(e.start, e.end);
  if (!inner.contains('\n')) return toggleWrap(e, '`');
  final t = e.text;
  if (inner.startsWith('```\n') && inner.endsWith('\n```')) {
    final code = inner.substring(4, inner.length - 4);
    return (text: t.replaceRange(e.start, e.end, code), start: e.start, end: e.start + code.length);
  }
  final before = e.start > 0 && t[e.start - 1] != '\n' ? '\n' : '';
  final after = e.end < t.length && t[e.end] != '\n' ? '\n' : '';
  final block = '$before```\n$inner\n```$after';
  final start = e.start + before.length + 4;
  return (text: t.replaceRange(e.start, e.end, block), start: start, end: start + inner.length);
}

/// The styles around a position: for showing which toolbar buttons are on.
Set<Mark> marksAt(String s, int start, int end) {
  final out = <Mark>{};
  void walk(List<Inline> nodes) {
    for (final n in nodes) {
      if (n is Styled && n.contentStart <= start && end <= n.contentEnd) {
        out.add(n.mark);
        walk(n.children);
      }
    }
  }

  for (final b in parseBlocks(s)) {
    switch (b) {
      case Paragraph():
        walk(b.inline);
      case Quote():
        for (final l in b.lines) {
          walk(l.inline);
        }
      case CodeBlock():
        break;
    }
  }
  return out;
}

// ---------------------------------------------------------------- mentions

/// The usernames a message mentions, each once, in order: the same as the
/// core finds.
List<String> mentionsIn(String s) {
  final out = <String>[];
  void walk(List<Inline> nodes) {
    for (final n in nodes) {
      switch (n) {
        case Mention():
          final u = n.username(s);
          if (!out.contains(u)) out.add(u);
        case Styled():
          walk(n.children);
        case Plain() || Escape() || Code():
          break;
      }
    }
  }

  for (final b in parseBlocks(s)) {
    switch (b) {
      case Paragraph():
        walk(b.inline);
      case Quote():
        for (final l in b.lines) {
          walk(l.inline);
        }
      case CodeBlock():
        break;
    }
  }
  return out;
}

/// The `@` and partial username the cursor is at the end of, while typing
/// a mention: where the `@` is and what follows it so far (perhaps
/// nothing). Null when the cursor is not in one.
({int at, String query})? mentionTyped(String s, int cursor) {
  if (cursor < 1 || cursor > s.length) return null;
  var k = cursor;
  while (k > 0 && _isUsernameChar(s.codeUnitAt(k - 1))) {
    k--;
  }
  final at = k - 1;
  if (at < 0 || s[at] != '@' || _isNameChar(s, at - 1)) return null;
  final query = s.substring(k, cursor);
  if (query.length > 32 || (query.isNotEmpty && !_isLower(query.codeUnitAt(0)))) return null;
  // Not inside code, where it would not be a mention.
  if (_inCode(s, at)) return null;
  return (at: at, query: query);
}

bool _inCode(String s, int i) {
  bool inInline(List<Inline> nodes) => nodes.any(
    (n) => switch (n) {
      Code() => n.start <= i && i < n.end,
      Styled() => n.start <= i && i < n.end && inInline(n.children),
      _ => false,
    },
  );
  for (final b in parseBlocks(s)) {
    if (i < b.start || i > b.end) continue;
    switch (b) {
      case CodeBlock():
        return true;
      case Paragraph():
        return inInline(b.inline);
      case Quote():
        return b.lines.any((l) => inInline(l.inline));
    }
  }
  return false;
}

// ---------------------------------------------------------------- plain

/// The text as read, without formatting markers: for previews and copying.
/// Spoilers stay hidden unless `revealSpoilers`.
String plainText(String s, {bool revealSpoilers = false}) {
  final b = StringBuffer();
  void inline(List<Inline> nodes) {
    for (final n in nodes) {
      switch (n) {
        case Plain() || Mention():
          b.write(s.substring(n.start, n.end));
        case Escape():
          b.write(s[n.end - 1]);
        case Code():
          b.write(s.substring(n.contentStart, n.contentEnd));
        case Styled(mark: Mark.spoiler) when !revealSpoilers:
          b.write('▒▒▒');
        case Styled():
          inline(n.children);
      }
    }
  }

  final blocks = parseBlocks(s);
  for (final (i, block) in blocks.indexed) {
    if (i > 0) b.write('\n');
    switch (block) {
      case Paragraph():
        inline(block.inline);
      case Quote():
        for (final (j, l) in block.lines.indexed) {
          if (j > 0) b.write('\n');
          inline(l.inline);
        }
      case CodeBlock():
        b.write(s.substring(block.codeStart, block.codeEnd));
    }
  }
  return b.toString();
}
