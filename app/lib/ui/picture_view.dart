// Profile pictures and banners on screen. Each is drawn at exactly the
// device pixels it fills, from a copy the core scaled once for that size
// with a sharp filter in linear light (see `picture::scaled` in the core),
// the way Discord's image servers and Signal's thumbnailing do it. Letting
// the GPU sample a 512-pixel picture into 32 pixels instead skips most of
// its pixels, and small pictures come out crunchy.
//
// Drawn as a filled shape, not cut out with a clip, and with nothing
// behind it: a clip's edge is not smoothed on every renderer, and a colour
// behind shows as a thin ring around the edge.

import 'dart:async';
import 'dart:ui' as ui;

import 'package:flutter/widgets.dart';
import 'package:post/src/rust/api/client.dart';

/// Scaled copies of pictures, by picture and size, ready to draw. The most
/// recently used few hundred are kept.
class PictureCache {
  PictureCache(this._client);

  final Client _client;
  static const _keep = 400;
  // Map literals keep insertion order: least recently used first.
  final _done = <String, ui.Image>{};
  final _pending = <String, Future<ui.Image?>>{};
  final _failed = <String>{};

  static String _key(String id, int w, int h) => '$id@${w}x$h';

  /// [id] at [w]×[h] pixels, if made already.
  ui.Image? now(String id, int w, int h) {
    final k = _key(id, w, h);
    final image = _done.remove(k);
    // Most recently used last.
    if (image != null) _done[k] = image;
    return image;
  }

  /// Some other size of [id] already made, to show while the right one is.
  ui.Image? anySize(String id) {
    for (final e in _done.entries.toList().reversed) {
      if (e.key.startsWith('$id@')) return e.value;
    }
    return null;
  }

  /// Make [id] at [w]×[h] pixels. Null if this device does not have it.
  Future<ui.Image?> load(String id, int w, int h) {
    final k = _key(id, w, h);
    if (_failed.contains(k)) return Future.value();
    return _pending[k] ??= _make(id, w, h, k);
  }

  Future<ui.Image?> _make(String id, int w, int h, String k) async {
    try {
      final rgba = await _client.pictureScaled(id: id, width: w, height: h);
      if (rgba == null) {
        _failed.add(k);
        return null;
      }
      final made = Completer<ui.Image>();
      ui.decodeImageFromPixels(rgba, w, h, ui.PixelFormat.rgba8888, made.complete);
      final image = await made.future;
      _done[k] = image;
      while (_done.length > _keep) {
        _done.remove(_done.keys.first);
      }
      return image;
    } catch (_) {
      _failed.add(k);
      return null;
    } finally {
      _pending.remove(k);
    }
  }
}

/// A picture to draw: which, and where its scaled copies come from.
@immutable
class PictureRef {
  const PictureRef(this.id, this.cache);

  final String id;
  final PictureCache cache;

  @override
  bool operator ==(Object other) => other is PictureRef && other.id == id && identical(other.cache, cache);

  @override
  int get hashCode => id.hashCode;
}

/// [picture] filling its box, from its middle, as a circle or a rectangle
/// with [radius]; [placeholder] until it is ready.
class PictureView extends StatefulWidget {
  const PictureView({super.key, required this.picture, this.circle = false, this.radius = BorderRadius.zero, this.placeholder});

  final PictureRef picture;
  final bool circle;
  final BorderRadius radius;
  final Widget? placeholder;

  @override
  State<PictureView> createState() => _PictureViewState();
}

class _PictureViewState extends State<PictureView> {
  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, box) {
        final dpr = MediaQuery.devicePixelRatioOf(context);
        final w = (box.maxWidth * dpr).round().clamp(1, 4096);
        final h = (box.maxHeight * dpr).round().clamp(1, 4096);
        final cache = widget.picture.cache;
        final id = widget.picture.id;
        var image = cache.now(id, w, h);
        if (image == null) {
          unawaited(
            cache.load(id, w, h).then((made) {
              if (made != null && mounted) setState(() {});
            }),
          );
          image = cache.anySize(id);
        }
        if (image == null) return widget.placeholder ?? const SizedBox.expand();
        return CustomPaint(
          size: Size(box.maxWidth, box.maxHeight),
          painter: _PicturePainter(image, circle: widget.circle, radius: widget.radius),
        );
      },
    );
  }
}

/// A banner: [picture] filling the box with rounded corners, or [color]
/// with none (and while it is made).
class BannerView extends StatelessWidget {
  const BannerView({super.key, required this.picture, required this.color, required this.radius});

  final PictureRef? picture;
  final Color color;
  final BorderRadius radius;

  @override
  Widget build(BuildContext context) {
    final plain = DecoratedBox(
      decoration: BoxDecoration(color: color, borderRadius: radius),
    );
    final p = picture;
    return p == null ? plain : PictureView(picture: p, radius: radius, placeholder: plain);
  }
}

class _PicturePainter extends CustomPainter {
  const _PicturePainter(this.image, {required this.circle, required this.radius});

  final ui.Image image;
  final bool circle;
  final BorderRadius radius;

  @override
  void paint(Canvas canvas, Size size) {
    final rect = Offset.zero & size;
    // Pixel for pixel when it was made for this size; scaled while a copy
    // of another size stands in.
    final fit = Matrix4.diagonal3Values(size.width / image.width, size.height / image.height, 1);
    final paint = Paint()..shader = ImageShader(image, TileMode.clamp, TileMode.clamp, fit.storage, filterQuality: FilterQuality.medium);
    if (circle) {
      canvas.drawOval(rect, paint);
    } else {
      canvas.drawRRect(radius.toRRect(rect), paint);
    }
  }

  @override
  bool shouldRepaint(_PicturePainter old) => old.image != image || old.circle != circle || old.radius != radius;
}
