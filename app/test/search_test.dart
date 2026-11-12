import 'package:flutter_test/flutter_test.dart';
import 'package:post/state/search_spec.dart';

void main() {
  test('filters and words, read and written', () {
    final s = SearchSpec.parse('rope from:@Alice has:link noon before:2026-11-01');
    expect(s.words, 'rope noon');
    expect(s.filters, const [SearchFilter(FilterKind.from, 'alice'), SearchFilter(FilterKind.has, 'link'), SearchFilter(FilterKind.before, '2026-11-01')]);
    expect('$s', 'from:alice has:link before:2026-11-01 rope noon');
    expect(SearchSpec.parse('$s'), s);
  });

  test('mentions: names who a message mentions', () {
    final s = SearchSpec.parse('mentions:@Sam rope');
    expect(s.filters, const [SearchFilter(FilterKind.mentions, 'sam')]);
    expect(s.options(oldestFirst: false, page: 0, pageSize: 25).mentions, 'sam');
    expect(SearchSpec.parse('mentions:').words, 'mentions:');
  });

  test('what is not a usable filter is a word', () {
    expect(SearchSpec.parse('from: has:video before:2026-02-30 to:x').words, 'from: has:video before:2026-02-30 to:x');
    expect(SearchSpec.parse('   ').isEmpty, isTrue);
  });

  test('dates become a range of whole local days', () {
    final o = SearchSpec.parse('during:2026-03-05 rope').options(oldestFirst: false, page: 2, pageSize: 25);
    expect(o.afterMs, BigInt.from(DateTime(2026, 3, 5).millisecondsSinceEpoch));
    expect(o.beforeMs, BigInt.from(DateTime(2026, 3, 6).millisecondsSinceEpoch));
    expect(o.offset, 50);
    // after: means from the next day.
    final a = SearchSpec.parse('after:2026-03-05').options(oldestFirst: true, page: 0, pageSize: 25);
    expect(a.afterMs, BigInt.from(DateTime(2026, 3, 6).millisecondsSinceEpoch));
    expect(a.beforeMs, isNull);
  });
}
