import 'package:flutter_test/flutter_test.dart';
import 'package:post/screens/search.dart';

void main() {
  test('words and a sender, in any order', () {
    expect(parseSearch('rope  noon'), (words: 'rope noon', from: null));
    expect(parseSearch('from:Alice rope'), (words: 'rope', from: 'alice'));
    expect(parseSearch('rope from:@bob noon'), (words: 'rope noon', from: 'bob'));
    // A bare "from:" is just a word.
    expect(parseSearch('from:'), (words: 'from:', from: null));
  });
}
