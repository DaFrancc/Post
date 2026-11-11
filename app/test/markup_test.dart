import 'package:flutter/painting.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:postern/ui/markup.dart';
import 'package:postern/ui/markup_text.dart';

/// Each plain run with the marks around it, as `text:mark,mark` (records
/// holding Sets would compare by identity).
List<String> runs(String s) {
  final out = <String>[];
  void add(String text, Set<Mark> marks) => out.add('$text:${(marks.map((m) => m.name).toList()..sort()).join(',')}');
  void walk(List<Inline> nodes, Set<Mark> marks) {
    for (final n in nodes) {
      switch (n) {
        case Plain():
          add(s.substring(n.start, n.end), marks);
        case Escape():
          add(s[n.end - 1], marks);
        case Code():
          add('`${s.substring(n.contentStart, n.contentEnd)}`', marks);
        case Styled():
          walk(n.children, {...marks, n.mark});
      }
    }
  }

  walk(parseInline(s), {});
  return out;
}

Edit sel(String text, [int? start, int? end]) => (text: text, start: start ?? 0, end: end ?? text.length);

void main() {
  group('inline', () {
    test('each style', () {
      expect(runs('**b** *i* _i_ __u__ ~~s~~ ||p||'), ['b:bold', ' :', 'i:italic', ' :', 'i:italic', ' :', 'u:underline', ' :', 's:strike', ' :', 'p:spoiler']);
    });

    test('nesting, and a run of three', () {
      expect(runs('***both***'), ['both:bold,italic']);
      expect(runs('*a **b** c*'), ['a :italic', 'b:bold,italic', ' c:italic']);
      expect(runs('||**secret**||'), ['secret:bold,spoiler']);
    });

    test('what is not formatting stays as typed', () {
      // Underscores inside words are not italic (as in other chat apps,
      // __init__ on its own is underline).
      expect(plainText('snake_case and file_name_here.py'), 'snake_case and file_name_here.py');
      expect(plainText('2 * 3 * 4'), '2 * 3 * 4');
      expect(plainText('**unclosed'), '**unclosed');
      expect(plainText('** spaced **'), '** spaced **');
      expect(plainText(r'\*not italic\*'), '*not italic*');
    });

    test('code keeps markers literal', () {
      expect(runs('`**x**` and **y**'), ['`**x**`:', ' and :', 'y:bold']);
      expect(plainText('``a ` b``'), 'a ` b');
      expect(plainText('```inline```'), 'inline');
    });
  });

  group('blocks', () {
    test('quotes and code blocks', () {
      const s = 'hi\n> quoted **bold**\n> more\n```dart\nvar x = **1**;\n```\nbye';
      final blocks = parseBlocks(s);
      expect(blocks.map((b) => b.runtimeType.toString()), ['Paragraph', 'Quote', 'CodeBlock', 'Paragraph']);
      final code = blocks[2] as CodeBlock;
      expect(code.language, 'dart');
      expect(s.substring(code.codeStart, code.codeEnd), 'var x = **1**;');
      expect(plainText(s), 'hi\nquoted bold\nmore\nvar x = **1**;\nbye');
    });

    test('an unclosed fence is text', () {
      expect(parseBlocks('```\nno end').single, isA<Paragraph>());
    });

    test('spoilers stay hidden in previews', () {
      expect(plainText('the end: ||he lives||'), 'the end: ▒▒▒');
      expect(plainText('the end: ||he lives||', revealSpoilers: true), 'the end: he lives');
    });
  });

  test('the composer styles every character, in order, changing none', () {
    // The editable text must be exactly what was typed, or the cursor and
    // selection would land in the wrong places.
    const samples = [
      '',
      'plain',
      '**b** *i* __u__ ~~s~~ `c` ||p|| \\* x',
      '***x*** *a **b** c* **unclosed',
      'line\n> quote **b**\n>\n```js\ncode **x**\n```\nafter',
      '```\nunclosed fence\n> q',
      '😀 **émoji** ||ü||',
    ];
    for (final s in samples) {
      final segs = editingSegments(s, const TextStyle());
      expect(segs.map((g) => s.substring(g.start, g.end)).join(), s, reason: s);
      for (var i = 1; i < segs.length; i++) {
        expect(segs[i].start, segs[i - 1].end, reason: 'contiguous in $s');
      }
    }
  });

  group('formatting commands', () {
    test('wrap and unwrap', () {
      final wrapped = toggleWrap(sel('say hi now', 4, 6), '**');
      expect(wrapped, (text: 'say **hi** now', start: 6, end: 8));
      expect(toggleWrap(wrapped, '**'), (text: 'say hi now', start: 4, end: 6));
      // Selecting the markers too unwraps.
      expect(toggleWrap(sel('say **hi** now', 4, 10), '**'), (text: 'say hi now', start: 4, end: 6));
      // Nothing selected: markers with the cursor between.
      expect(toggleWrap(sel('ab', 1, 1), '~~'), (text: 'a~~~~b', start: 3, end: 3));
    });

    test('italic is not bold', () {
      // The `*` inside `**` does not count as italic around the selection.
      expect(toggleWrap(sel('**hi**', 2, 4), '*').text, '***hi***');
    });

    test('quote lines', () {
      final q = toggleQuote(sel('one\ntwo', 0, 7));
      expect(q.text, '> one\n> two');
      expect(toggleQuote(q).text, 'one\ntwo');
    });

    test('code: inline on one line, a block over several', () {
      expect(toggleCode(sel('a b', 2, 3)).text, 'a `b`');
      final block = toggleCode(sel('x\ny', 0, 3));
      expect(block.text, '```\nx\ny\n```');
      expect(block.text.substring(block.start, block.end), 'x\ny');
    });

    test('which marks are on', () {
      expect(marksAt('**bold *both***', 8, 12), {Mark.bold, Mark.italic});
      expect(marksAt('plain', 0, 5), <Mark>{});
    });
  });
}
