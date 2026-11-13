// Choosing a profile picture or banner: pick an image file, frame it (drag
// to move, scroll or the slider to zoom), and save. The core shrinks it to
// fit the limits, encrypts it and shares it; nothing here touches the file
// beyond showing it.

import 'dart:async';
import 'dart:io';
import 'dart:math' as math;
import 'dart:ui' as ui;

import 'package:file_selector/file_selector.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:post/src/rust/api/client.dart';

import '../state/app_state.dart';
import '../theme.dart';
import '../ui/controls.dart';

const _images = XTypeGroup(label: 'Images', extensions: ['png', 'jpg', 'jpeg', 'webp', 'gif']);

/// What a picture of [kind] may be, in words: "512 × 512 and 200 KB".
String pictureLimitsText(PictureKind kind) {
  final l = Client.pictureLimits(kind: kind);
  return '${l.maxWidth} × ${l.maxHeight} and ${l.maxBytes ~/ 1024} KB';
}

/// Pick an image and set it as the user's picture of [kind]. Resolves
/// true once it is saved and shared.
Future<bool> choosePicture(BuildContext context, AppState state, PictureKind kind) async {
  final picked = await openFile(acceptedTypeGroups: [_images]);
  if (picked == null || !context.mounted) return false;
  final limits = Client.pictureLimits(kind: kind);
  final ui.Image image;
  try {
    final file = File(picked.path);
    if (await file.length() > limits.maxSourceBytes) {
      if (context.mounted) showToast(context, 'That image is over ${limits.maxSourceBytes ~/ (1024 * 1024)} MB. Pick a smaller one.', error: true);
      return false;
    }
    image = await decodeImageFromList(await file.readAsBytes());
  } catch (_) {
    if (context.mounted) showToast(context, 'That file is not an image Post can read (PNG, JPEG, WebP or GIF).', error: true);
    return false;
  }
  if (!context.mounted) return false;
  final saved = await showDialog<bool>(
    context: context,
    builder: (_) => _FrameDialog(state: state, kind: kind, path: picked.path, image: image),
  );
  image.dispose();
  if (saved == true) await state.reloadPerson(state.me.username);
  return saved == true;
}

/// Remove the user's picture of [kind].
Future<void> removePicture(BuildContext context, AppState state, PictureKind kind) async {
  try {
    await state.client.removePicture(kind: kind);
    await state.reloadPerson(state.me.username);
  } catch (e) {
    if (context.mounted) showToast(context, errorText(e), error: true);
  }
}

class _FrameDialog extends StatefulWidget {
  const _FrameDialog({required this.state, required this.kind, required this.path, required this.image});

  final AppState state;
  final PictureKind kind;
  final String path;
  final ui.Image image;

  @override
  State<_FrameDialog> createState() => _FrameDialogState();
}

class _FrameDialogState extends State<_FrameDialog> {
  static const _maxZoom = 4.0;

  /// How far in, 1 being the whole frame just covered.
  double _zoom = 1;

  /// The image's top left corner within the frame, in frame pixels.
  Offset _offset = Offset.zero;
  bool _placed = false;
  double _zoomAtStart = 1;
  bool _busy = false;
  String? _error;

  bool get _avatar => widget.kind == PictureKind.avatar;

  /// The frame on screen: the shape of the picture.
  Size get _frame => _avatar ? const Size(300, 300) : const Size(456, 152);

  Size get _imageSize => Size(widget.image.width.toDouble(), widget.image.height.toDouble());

  /// Frame pixels per image pixel.
  double get _scale {
    final f = _frame;
    final i = _imageSize;
    return math.max(f.width / i.width, f.height / i.height) * _zoom;
  }

  /// [offset] moved as little as it takes for the image to cover the frame.
  Offset _clamped(Offset offset) {
    final f = _frame;
    final shown = _imageSize * _scale;
    return Offset(offset.dx.clamp(f.width - shown.width, 0.0), offset.dy.clamp(f.height - shown.height, 0.0));
  }

  /// Zoom to [zoom], keeping the image point under [focus] (frame
  /// coordinates) where it is.
  void _zoomTo(double zoom, Offset focus) {
    final before = _scale;
    final z = zoom.clamp(1.0, _maxZoom);
    setState(() {
      _zoom = z;
      final after = _scale;
      _offset = _clamped(focus - (focus - _offset) * (after / before));
    });
  }

  PictureCrop get _crop {
    final shown = _imageSize * _scale;
    final f = _frame;
    return PictureCrop(x: -_offset.dx / shown.width, y: -_offset.dy / shown.height, width: f.width / shown.width, height: f.height / shown.height);
  }

  Future<void> _save() async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await widget.state.client.setPicture(kind: widget.kind, path: widget.path, crop: _crop);
      if (mounted) Navigator.of(context).pop(true);
    } catch (e) {
      if (mounted) {
        setState(() {
          _busy = false;
          _error = errorText(e);
        });
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    if (!_placed) {
      // Start centred.
      final shown = _imageSize * _scale;
      _offset = Offset((_frame.width - shown.width) / 2, (_frame.height - shown.height) / 2);
      _placed = true;
    }
    final shown = _imageSize * _scale;
    return PDialog(
      title: _avatar ? 'Profile picture' : 'Banner',
      width: 540,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(
            'Drag to move it, scroll to zoom. It is shrunk to ${pictureLimitsText(widget.kind)}.',
            style: const TextStyle(fontSize: 13, color: PColors.textMuted),
          ),
          const SizedBox(height: 16),
          Center(
            child: Listener(
              onPointerSignal: (e) {
                if (e is PointerScrollEvent && !_busy) _zoomTo(_zoom * math.pow(0.998, e.scrollDelta.dy), e.localPosition);
              },
              child: GestureDetector(
                onScaleStart: (_) => _zoomAtStart = _zoom,
                onScaleUpdate: _busy
                    ? null
                    : (d) {
                        if (d.scale != 1) {
                          _zoomTo(_zoomAtStart * d.scale, d.localFocalPoint);
                        }
                        setState(() => _offset = _clamped(_offset + d.focalPointDelta));
                      },
                child: MouseRegion(
                  cursor: SystemMouseCursors.move,
                  // A square clip: its straight edges need no smoothing.
                  child: ClipRect(
                    child: SizedBox.fromSize(
                      size: _frame,
                      child: Stack(
                        children: [
                          Positioned(
                            left: _offset.dx,
                            top: _offset.dy,
                            width: shown.width,
                            height: shown.height,
                            child: RawImage(image: widget.image, fit: BoxFit.fill, filterQuality: FilterQuality.medium),
                          ),
                          // What a round picture keeps: outside the circle dims.
                          if (_avatar) const Positioned.fill(child: CustomPaint(painter: _RoundMask())),
                        ],
                      ),
                    ),
                  ),
                ),
              ),
            ),
          ),
          const SizedBox(height: 14),
          Row(
            children: [
              const Icon(Icons.photo_size_select_small_rounded, size: 18, color: PColors.textFaint),
              Expanded(
                child: Slider(
                  value: _zoom,
                  min: 1,
                  max: _maxZoom,
                  onChanged: _busy ? null : (z) => _zoomTo(z, _frame.center(Offset.zero)),
                ),
              ),
              const Icon(Icons.photo_size_select_large_rounded, size: 22, color: PColors.textFaint),
            ],
          ),
          if (_error != null) ...[const SizedBox(height: 8), PErrorBox(_error!)],
          PDialogActions(
            children: [
              OutlinedButton(onPressed: _busy ? null : () => Navigator.of(context).pop(false), child: const Text('Cancel')),
              FilledButton(onPressed: _busy ? null : _save, child: _busy ? const PSpinner() : const Text('Save')),
            ],
          ),
        ],
      ),
    );
  }
}

/// Dims all but the circle a round picture shows, and rings it.
class _RoundMask extends CustomPainter {
  const _RoundMask();

  @override
  void paint(Canvas canvas, Size size) {
    final circle = Path()..addOval(Offset.zero & size);
    final outside = Path.combine(PathOperation.difference, Path()..addRect(Offset.zero & size), circle);
    canvas.drawPath(outside, Paint()..color = const Color(0xA6000000));
    canvas.drawPath(
      circle,
      Paint()
        ..style = PaintingStyle.stroke
        ..strokeWidth = 2
        ..color = const Color(0xCCFFFFFF),
    );
  }

  @override
  bool shouldRepaint(_RoundMask old) => false;
}
