import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:post/screens/settings.dart';

TextEditingValue _typed(String text) => TextEditingValue(
  text: text,
  selection: TextSelection.collapsed(offset: text.length),
);

void main() {
  final f = BioFormatter(10);

  test('a bio keeps line breaks; tabs become spaces; other control characters go', () {
    final v = f.formatEditUpdate(TextEditingValue.empty, _typed('a\tb\r\nc\u0007'));
    expect(v.text, 'a b\nc');
    expect(v.selection.baseOffset, v.text.length, reason: 'the cursor stays at the end of what was typed');
  });

  test('characters that reorder text are dropped', () {
    // A right-to-left override and an isolate, by code: never literally in
    // source, where they would hide what the code says.
    final override = String.fromCharCode(0x202E), isolate = String.fromCharCode(0x2066);
    expect(BioFormatter.clean('ab${override}cd${isolate}e'), 'abcde');
  });

  test('characters count as runes, as the core counts them', () {
    // An emoji is two UTF-16 units but one character.
    final emoji = '😀' * 10;
    expect(f.formatEditUpdate(TextEditingValue.empty, _typed(emoji)).text, emoji);
  });

  test('typing past the limit does nothing; a paste is cut to fit', () {
    final full = _typed('0123456789');
    expect(f.formatEditUpdate(full, _typed('0123456789x')), full);
    final cut = f.formatEditUpdate(_typed('01234'), _typed('01234pasted text'));
    expect(cut.text, '01234paste');
    expect(cut.selection.baseOffset, 10);
  });
}
