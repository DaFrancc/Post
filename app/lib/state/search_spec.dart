// A search as the user builds it: words, plus filters shown as chips in the
// search box (`from: alice`, `has: link`, `before: 2026-11-01`). Written
// out as text the same way, so a search can be typed, kept in the history
// and read back.

import 'package:post/src/rust/api/client.dart';

enum FilterKind {
  from('From a specific user', 'user'),
  has('Includes a link', 'link'),
  before('Before a date', 'date'),
  after('After a date', 'date'),
  during('On a date', 'date');

  const FilterKind(this.title, this.hint);

  /// How the filter is offered, and what its value is.
  final String title;
  final String hint;

  static FilterKind? named(String s) {
    for (final k in values) {
      if (k.name == s) return k;
    }
    return null;
  }
}

class SearchFilter {
  const SearchFilter(this.kind, this.value);

  final FilterKind kind;
  final String value;

  /// As written and as shown on its chip.
  @override
  String toString() => '${kind.name}:$value';

  @override
  bool operator ==(Object other) => other is SearchFilter && other.kind == kind && other.value == value;

  @override
  int get hashCode => Object.hash(kind, value);

  /// A date filter's day, if its value is one (YYYY-MM-DD).
  DateTime? get day {
    final m = RegExp(r'^(\d{4})-(\d{2})-(\d{2})$').firstMatch(value);
    if (m == null) return null;
    final d = DateTime(int.parse(m[1]!), int.parse(m[2]!), int.parse(m[3]!));
    return d.month == int.parse(m[2]!) && d.day == int.parse(m[3]!) ? d : null;
  }

  /// Whether this filter can be searched with.
  bool get valid => switch (kind) {
    FilterKind.from => value.isNotEmpty,
    FilterKind.has => value == 'link',
    _ => day != null,
  };
}

/// A day as filters write it.
String dayText(DateTime d) => '${d.year.toString().padLeft(4, '0')}-${d.month.toString().padLeft(2, '0')}-${d.day.toString().padLeft(2, '0')}';

class SearchSpec {
  const SearchSpec({this.words = '', this.filters = const []});

  final String words;
  final List<SearchFilter> filters;

  bool get isEmpty => words.trim().isEmpty && filters.isEmpty;

  /// Read a search from text: `kind:value` for each filter (an `@` before a
  /// username is fine), the rest words.
  static SearchSpec parse(String text) {
    final words = <String>[];
    final filters = <SearchFilter>[];
    for (final t in text.split(RegExp(r'\s+')).where((t) => t.isNotEmpty)) {
      final colon = t.indexOf(':');
      final kind = colon > 0 ? FilterKind.named(t.substring(0, colon).toLowerCase()) : null;
      final value = colon > 0 ? t.substring(colon + 1).replaceFirst(RegExp('^@'), '').toLowerCase() : '';
      final f = kind == null ? null : SearchFilter(kind, value);
      if (f != null && f.valid) {
        filters.add(f);
      } else {
        words.add(t);
      }
    }
    return SearchSpec(words: words.join(' '), filters: filters);
  }

  @override
  String toString() => [...filters.map((f) => '$f'), if (words.trim().isNotEmpty) words.trim()].join(' ');

  @override
  bool operator ==(Object other) => other is SearchSpec && other.toString() == toString();

  @override
  int get hashCode => toString().hashCode;

  /// What to ask the core for: one page, in one conversation or all.
  SearchOptions options({String? conversation, required bool oldestFirst, required int page, required int pageSize}) {
    String? from;
    var hasLink = false;
    int? after;
    int? before;
    // Start of a day, and of the next, in local time.
    int start(DateTime d) => d.millisecondsSinceEpoch;
    int next(DateTime d) => DateTime(d.year, d.month, d.day + 1).millisecondsSinceEpoch;
    int? latest(int? a, int b) => a == null || b > a ? b : a;
    int? earliest(int? a, int b) => a == null || b < a ? b : a;
    for (final f in filters) {
      final d = f.day;
      switch (f.kind) {
        case FilterKind.from:
          from = f.value;
        case FilterKind.has:
          hasLink = true;
        case FilterKind.before when d != null:
          before = earliest(before, start(d));
        case FilterKind.after when d != null:
          after = latest(after, next(d));
        case FilterKind.during when d != null:
          after = latest(after, start(d));
          before = earliest(before, next(d));
        default:
          break;
      }
    }
    return SearchOptions(
      text: words,
      conversation: conversation,
      from: from,
      hasLink: hasLink,
      afterMs: after == null ? null : BigInt.from(after),
      beforeMs: before == null ? null : BigInt.from(before),
      oldestFirst: oldestFirst,
      limit: pageSize,
      offset: page * pageSize,
    );
  }
}
