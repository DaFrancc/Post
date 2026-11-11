// Controls shared by every screen: fields, icon buttons, avatars, dialogs
// and toasts, all drawn from the theme.

import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../theme.dart';
import 'format.dart';

/// A labelled text field with an error or help line under it.
class PField extends StatelessWidget {
  const PField({
    super.key,
    required this.label,
    required this.controller,
    this.hint,
    this.help,
    this.error,
    this.enabled = true,
    this.mono = false,
    this.autofocus = false,
    this.maxLength,
    this.onSubmitted,
    this.onChanged,
  });

  final String label;
  final TextEditingController controller;
  final String? hint;
  final String? help;
  final String? error;
  final bool enabled;
  final bool mono;
  final bool autofocus;
  final int? maxLength;
  final ValueChanged<String>? onSubmitted;
  final ValueChanged<String>? onChanged;

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(label.toUpperCase(), style: PType.label),
        const SizedBox(height: 8),
        TextField(
          controller: controller,
          enabled: enabled,
          autofocus: autofocus,
          onSubmitted: onSubmitted,
          onChanged: onChanged,
          autocorrect: false,
          maxLength: maxLength,
          maxLengthEnforcement: MaxLengthEnforcement.enforced,
          style: TextStyle(fontSize: 14.5, color: PColors.text, fontFamily: mono ? 'monospace' : PType.body),
          decoration: InputDecoration(
            hintText: hint,
            counterText: '',
            enabledBorder: error == null
                ? null
                : OutlineInputBorder(
                    borderRadius: BorderRadius.circular(PShape.controlRadius),
                    borderSide: const BorderSide(color: PColors.danger),
                  ),
          ),
        ),
        if (error != null) ...[
          const SizedBox(height: 6),
          Text(error!, style: const TextStyle(fontSize: 12.5, height: 1.4, color: PColors.dangerText)),
        ] else if (help != null) ...[
          const SizedBox(height: 6),
          Text(help!, style: PType.small.copyWith(height: 1.4)),
        ],
      ],
    );
  }
}

/// A small busy indicator, for inside buttons.
class PSpinner extends StatelessWidget {
  const PSpinner({super.key, this.size = 18});

  final double size;

  @override
  Widget build(BuildContext context) => SizedBox(
    width: size,
    height: size,
    child: const CircularProgressIndicator(strokeWidth: 2, color: PColors.accentText),
  );
}

/// An error, said plainly, in a tinted box.
class PErrorBox extends StatelessWidget {
  const PErrorBox(this.text, {super.key});

  final String text;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.all(12),
      decoration: BoxDecoration(color: PColors.dangerSoft, borderRadius: BorderRadius.circular(12)),
      child: Text(text, style: const TextStyle(fontSize: 13, height: 1.4, color: PColors.dangerText)),
    );
  }
}

/// A square icon button that lights up on hover.
class PIconButton extends StatefulWidget {
  const PIconButton({
    super.key,
    required this.icon,
    required this.tooltip,
    required this.onPressed,
    this.size = 36,
    this.iconSize = 18,
    this.selected = false,
    this.color,
  });

  final IconData icon;
  final String tooltip;
  final VoidCallback? onPressed;
  final double size;
  final double iconSize;
  final bool selected;
  final Color? color;

  @override
  State<PIconButton> createState() => _PIconButtonState();
}

class _PIconButtonState extends State<PIconButton> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final enabled = widget.onPressed != null;
    final lit = enabled && (_hover || widget.selected);
    return Tooltip(
      message: widget.tooltip,
      waitDuration: const Duration(milliseconds: 500),
      child: MouseRegion(
        cursor: enabled ? SystemMouseCursors.click : SystemMouseCursors.basic,
        onEnter: (_) => setState(() => _hover = true),
        onExit: (_) => setState(() => _hover = false),
        child: Semantics(
          button: true,
          label: widget.tooltip,
          child: GestureDetector(
            onTap: widget.onPressed,
            child: AnimatedContainer(
              duration: const Duration(milliseconds: 120),
              width: widget.size,
              height: widget.size,
              decoration: BoxDecoration(
                color: widget.selected ? PColors.active : (lit ? PColors.hoverStrong : Colors.transparent),
                borderRadius: BorderRadius.circular(PShape.controlRadius),
              ),
              child: Icon(
                widget.icon,
                size: widget.iconSize,
                color: widget.color ?? (!enabled ? PColors.textFaint : (widget.selected ? PColors.accentText : (lit ? PColors.text : PColors.textMuted))),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// Someone's (or a group's) initials on their colour, optionally with a
/// presence dot. Groups are rounded squares, people are circles.
class PAvatar extends StatelessWidget {
  const PAvatar({super.key, required this.name, required this.colorKey, this.size = 36, this.square = false, this.presence, this.ringColor = PColors.panel});

  final String name;
  final String colorKey;
  final double size;
  final bool square;

  /// A presence word (online, away, dnd, offline), or null for no dot.
  final String? presence;

  /// The colour behind the avatar, so the dot looks cut out of it.
  final Color ringColor;

  @override
  Widget build(BuildContext context) {
    final face = Container(
      width: size,
      height: size,
      alignment: Alignment.center,
      decoration: BoxDecoration(color: PColors.avatarFor(colorKey), borderRadius: BorderRadius.circular(square ? size * 0.33 : size / 2)),
      child: Text(
        initials(name),
        style: TextStyle(fontFamily: square ? PType.display : PType.body, fontWeight: FontWeight.w600, fontSize: size * 0.36, color: PColors.onAvatar),
      ),
    );
    final p = presence;
    if (p == null) return face;
    // Fixed sizes for a fixed-size avatar: the dot belongs to the picture,
    // not the text.
    final dot = (size * 0.28).clamp(9.0, 16.0);
    final ring = (size * 0.075).clamp(2.5, 4.0);
    return SizedBox(
      width: size,
      height: size,
      child: Stack(
        clipBehavior: Clip.none,
        children: [
          face,
          Positioned(
            right: -ring + size * 0.02,
            bottom: -ring + size * 0.02,
            child: Container(
              width: dot + 2 * ring,
              height: dot + 2 * ring,
              decoration: BoxDecoration(color: ringColor, shape: BoxShape.circle),
              alignment: Alignment.center,
              child: PresenceDot(p, size: dot),
            ),
          ),
        ],
      ),
    );
  }
}

/// A coloured presence dot. Offline is a hollow ring, so it reads without
/// colour.
class PresenceDot extends StatelessWidget {
  const PresenceDot(this.word, {super.key, this.size = 10});

  final String word;
  final double size;

  @override
  Widget build(BuildContext context) {
    final offline = word != 'online' && word != 'away' && word != 'dnd';
    return AnimatedContainer(
      duration: PMotion.slow,
      width: size,
      height: size,
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        color: offline ? Colors.transparent : PColors.presence(word),
        border: offline ? Border.all(color: PColors.offline, width: size * 0.25) : null,
      ),
    );
  }
}

/// Around a dialog: blurs what is behind it, and lets it settle into place
/// as it fades in.
class PDialogEntrance extends StatelessWidget {
  const PDialogEntrance({super.key, required this.child});

  final Widget child;

  @override
  Widget build(BuildContext context) {
    return Stack(
      fit: StackFit.expand,
      children: [
        // Taps pass through to the barrier, which closes the dialog. Clipped,
        // or the blur would reach past the content to the title bar.
        IgnorePointer(
          child: ClipRect(
            child: BackdropFilter(filter: ui.ImageFilter.blur(sigmaX: 6, sigmaY: 6), child: const SizedBox.expand()),
          ),
        ),
        TweenAnimationBuilder<double>(
          tween: Tween(begin: 0.96, end: 1),
          duration: PMotion.normal,
          curve: PMotion.curve,
          builder: (context, scale, child) => Transform.scale(scale: scale, child: child),
          child: child,
        ),
      ],
    );
  }
}

/// A dialog in Postern's style: a title with a close button, then content.
class PDialog extends StatelessWidget {
  const PDialog({super.key, required this.title, required this.child, this.width = 460});

  final String title;
  final Widget child;
  final double width;

  @override
  Widget build(BuildContext context) {
    return PDialogEntrance(child: _dialog(context));
  }

  Widget _dialog(BuildContext context) {
    return Dialog(
      backgroundColor: PColors.dialog,
      insetPadding: const EdgeInsets.all(24),
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(22),
        side: const BorderSide(color: PColors.borderStrong),
      ),
      child: ConstrainedBox(
        constraints: BoxConstraints(maxWidth: width),
        child: Padding(
          padding: const EdgeInsets.fromLTRB(26, 20, 20, 26),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Row(
                children: [
                  Expanded(child: Text(title, style: PType.title)),
                  PIconButton(icon: Icons.close_rounded, tooltip: 'Close', onPressed: () => Navigator.of(context).pop()),
                ],
              ),
              const SizedBox(height: 14),
              Padding(padding: const EdgeInsets.only(right: 6), child: child),
            ],
          ),
        ),
      ),
    );
  }
}

/// A row of dialog buttons, right-aligned.
class PDialogActions extends StatelessWidget {
  const PDialogActions({super.key, required this.children});

  final List<Widget> children;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.only(top: 22),
      child: Wrap(alignment: WrapAlignment.end, spacing: 10, runSpacing: 10, children: children),
    );
  }
}

/// A compact menu entry: 32px (more with a caption), a soft rounded
/// highlight on hover. Destructive entries are red. An icon goes on the
/// right, only where it says something the label does not.
class PMenuItem<T> extends PopupMenuEntry<T> {
  const PMenuItem({super.key, required this.value, required this.label, this.caption, this.icon, this.danger = false});

  final T value;
  final String label;

  /// A short line under the label, like "Only you see it".
  final String? caption;
  final IconData? icon;
  final bool danger;

  @override
  double get height => caption == null ? 32 : 46;

  @override
  bool represents(T? v) => v == value;

  @override
  State<PMenuItem<T>> createState() => _PMenuItemState<T>();
}

class _PMenuItemState<T> extends State<PMenuItem<T>> {
  bool _hover = false;

  @override
  Widget build(BuildContext context) {
    final w = widget;
    final fg = w.danger ? PColors.danger : (_hover ? PColors.textStrong : PColors.text);
    return MouseRegion(
      cursor: SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hover = true),
      onExit: (_) => setState(() => _hover = false),
      child: GestureDetector(
        onTap: () => Navigator.of(context).pop<T>(w.value),
        child: AnimatedContainer(
          duration: PMotion.fast,
          height: w.height,
          padding: const EdgeInsets.symmetric(horizontal: 10),
          decoration: BoxDecoration(
            color: _hover ? (w.danger ? PColors.dangerSoft : PColors.active) : Colors.transparent,
            borderRadius: BorderRadius.circular(8),
          ),
          child: Row(
            children: [
              Expanded(
                child: Column(
                  mainAxisAlignment: MainAxisAlignment.center,
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      w.label,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 14, fontWeight: FontWeight.w500, color: fg),
                    ),
                    if (w.caption != null)
                      Text(
                        w.caption!,
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(fontSize: 12, color: PColors.textFaint),
                      ),
                  ],
                ),
              ),
              if (w.icon != null) ...[const SizedBox(width: 12), Icon(w.icon, size: 16, color: w.danger ? fg : PColors.textMuted)],
            ],
          ),
        ),
      ),
    );
  }
}

/// A thin line between groups of menu entries.
class PMenuDivider extends PopupMenuEntry<Never> {
  const PMenuDivider({super.key});

  @override
  double get height => 9;

  @override
  bool represents(void value) => false;

  @override
  State<PMenuDivider> createState() => _PMenuDividerState();
}

class _PMenuDividerState extends State<PMenuDivider> {
  @override
  Widget build(BuildContext context) => const Padding(
    padding: EdgeInsets.symmetric(horizontal: 6, vertical: 4),
    child: Divider(height: 1, thickness: 1, color: PColors.borderStrong),
  );
}

/// Open a context menu with its corner at the pointer.
Future<T?> showContextMenu<T>(BuildContext context, Offset globalPosition, List<PopupMenuEntry<T>> items) {
  return showMenu<T>(
    context: context,
    position: menuAt(context, globalPosition),
    constraints: const BoxConstraints(minWidth: 200, maxWidth: 280),
    menuPadding: const EdgeInsets.all(6),
    items: items,
  );
}

/// Where to open a context menu so its corner is at the pointer. Pointer
/// positions are in window coordinates; menus open in the navigator's
/// overlay, which sits inside the window frame (below the title bar, inside
/// the margin), so the point is converted.
RelativeRect menuAt(BuildContext context, Offset globalPosition) {
  final overlay = Overlay.of(context).context.findRenderObject()! as RenderBox;
  final at = overlay.globalToLocal(globalPosition);
  return RelativeRect.fromRect(at & const Size(1, 1), Offset.zero & overlay.size);
}

/// Ask before something that cannot be undone. Returns true to go ahead.
Future<bool> confirm(BuildContext context, {required String title, required String body, required String action, bool danger = false}) async {
  final ok = await showDialog<bool>(
    context: context,
    builder: (context) => PDialog(
      title: title,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(body, style: PType.text.copyWith(color: PColors.textMuted)),
          PDialogActions(
            children: [
              OutlinedButton(onPressed: () => Navigator.of(context).pop(false), child: const Text('Cancel')),
              FilledButton(
                autofocus: true,
                style: danger ? FilledButton.styleFrom(backgroundColor: PColors.danger, foregroundColor: PColors.onAccent) : null,
                onPressed: () => Navigator.of(context).pop(true),
                child: Text(action),
              ),
            ],
          ),
        ],
      ),
    ),
  );
  return ok ?? false;
}

/// A short message at the bottom of the window.
void showToast(BuildContext context, String text, {bool error = false}) {
  final messenger = ScaffoldMessenger.maybeOf(context);
  if (messenger == null) return;
  messenger.hideCurrentSnackBar();
  messenger.showSnackBar(
    SnackBar(
      behavior: SnackBarBehavior.floating,
      width: 480,
      backgroundColor: error ? PColors.dangerSoft : PColors.panelRaised,
      shape: RoundedRectangleBorder(
        borderRadius: BorderRadius.circular(14),
        side: BorderSide(color: error ? PColors.danger.withValues(alpha: 0.4) : PColors.borderStrong),
      ),
      content: Text(text, style: TextStyle(fontSize: 13.5, height: 1.4, color: error ? PColors.dangerText : PColors.text)),
      duration: Duration(seconds: error ? 6 : 3),
    ),
  );
}
