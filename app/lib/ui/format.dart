// Small text helpers: initials, times and presence words.

/// Up to two letters for an avatar: the first letters of the first two
/// words, or the first two letters of a single word.
String initials(String name) {
  final words = name.trim().split(RegExp(r'[\s_\-.]+')).where((w) => w.isNotEmpty).toList();
  if (words.isEmpty) return '?';
  if (words.length == 1) {
    final w = words.first.runes.toList();
    return String.fromCharCodes(w.take(2)).toUpperCase();
  }
  return (String.fromCharCode(words[0].runes.first) + String.fromCharCode(words[1].runes.first)).toUpperCase();
}

DateTime _local(int ms) => DateTime.fromMillisecondsSinceEpoch(ms);

String _two(int n) => n.toString().padLeft(2, '0');

const _weekdays = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'];
const _months = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

/// 24-hour clock time, like 09:41.
String clockTime(int ms) {
  final t = _local(ms);
  return '${_two(t.hour)}:${_two(t.minute)}';
}

bool sameDay(int a, int b) {
  final x = _local(a), y = _local(b);
  return x.year == y.year && x.month == y.month && x.day == y.day;
}

int _daysAgo(int ms) {
  final now = DateTime.now();
  final today = DateTime(now.year, now.month, now.day);
  final t = _local(ms);
  return today.difference(DateTime(t.year, t.month, t.day)).inDays;
}

/// A day, for separators in a conversation: Today, Yesterday, Monday, or
/// 3 Nov 2026.
String dayLabel(int ms) {
  final ago = _daysAgo(ms);
  final t = _local(ms);
  if (ago == 0) return 'Today';
  if (ago == 1) return 'Yesterday';
  if (ago > 1 && ago < 7) return _weekdays[t.weekday - 1];
  return '${t.day} ${_months[t.month - 1]} ${t.year}';
}

/// When a message was sent, next to the sender's name: the time today,
/// otherwise the day and time.
String messageTime(int ms) {
  final ago = _daysAgo(ms);
  if (ago == 0) return clockTime(ms);
  if (ago == 1) return 'Yesterday ${clockTime(ms)}';
  return '${dayLabel(ms)} ${clockTime(ms)}';
}

/// What others see of someone, as a label.
String presenceLabel(String word) => switch (word) {
      'online' => 'Online',
      'away' => 'Away',
      'dnd' => 'Do not disturb',
      _ => 'Offline',
    };
