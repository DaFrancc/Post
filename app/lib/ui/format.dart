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

const _monthNames = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'];

/// A moment written out in this device's time zone, for announcements:
/// "today at 02:00", "tomorrow at 02:00", "Thursday 14 November at 02:00"
/// (with the year when it is not this one).
String longWhen(int ms) {
  final t = _local(ms);
  final ago = _daysAgo(ms);
  final day = switch (ago) {
    0 => 'today',
    -1 => 'tomorrow',
    1 => 'yesterday',
    _ => '${_weekdays[t.weekday - 1]} ${t.day} ${_monthNames[t.month - 1]}${t.year == DateTime.now().year ? '' : ' ${t.year}'}',
  };
  return '$day at ${clockTime(ms)}';
}

/// A length of time in minutes, as words: "15 minutes", "1 hour 30
/// minutes", "2 days".
String minutesText(int minutes) {
  String unit(int n, String one) => '$n $one${n == 1 ? '' : 's'}';
  final (d, h, m) = (minutes ~/ 1440, minutes % 1440 ~/ 60, minutes % 60);
  return [if (d > 0) unit(d, 'day'), if (h > 0) unit(h, 'hour'), if (m > 0 || minutes == 0) unit(m, 'minute')].join(' ');
}

/// This device's time zone, for someone choosing a time others will see in
/// theirs: "CET (UTC+1)".
String timeZoneText([DateTime? at]) {
  final t = at ?? DateTime.now();
  final o = t.timeZoneOffset;
  final sign = o.isNegative ? '−' : '+';
  final h = o.inMinutes.abs() ~/ 60, m = o.inMinutes.abs() % 60;
  final utc = 'UTC$sign$h${m == 0 ? '' : ':${_two(m)}'}';
  return t.timeZoneName == utc || t.timeZoneName.isEmpty ? utc : '${t.timeZoneName} ($utc)';
}

/// What others see of someone, as a label.
String presenceLabel(String word) => switch (word) {
  'online' => 'Online',
  'away' => 'Away',
  'dnd' => 'Do not disturb',
  _ => 'Offline',
};
