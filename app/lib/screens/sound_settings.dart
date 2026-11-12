// Settings › Sounds: the sound pack, the volume, and each sound: hear it,
// swap it for a file of your own, or turn it off.

import 'dart:async';

import 'package:file_selector/file_selector.dart';
import 'package:flutter/material.dart';

import '../state/sounds.dart';
import '../theme.dart';
import '../ui/controls.dart';

class SoundSettings extends StatelessWidget {
  const SoundSettings({super.key, required this.sounds});

  final Sounds sounds;

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: sounds,
      builder: (context, _) => ListView(
        padding: const EdgeInsets.fromLTRB(28, 16, 28, 28),
        children: [
          Text('SOUND PACK', style: PType.label),
          const SizedBox(height: 8),
          _PackRow(sounds: sounds),
          const SizedBox(height: 6),
          const Text(
            'A pack is a folder of sound files. Files named after a sound (ring_incoming, message, call_ringing, mention3…) '
            'are used for it, or a pack.json names them. Anything the pack lacks plays Post\'s own.',
            style: PType.small,
          ),
          const SizedBox(height: 22),
          Text('VOLUME', style: PType.label),
          Row(
            children: [
              const Icon(Icons.volume_down_rounded, size: 18, color: PColors.textMuted),
              Expanded(
                child: Slider(
                  value: sounds.volume,
                  onChanged: sounds.setVolume,
                  onChangeEnd: (_) {
                    unawaited(sounds.saveVolume());
                    unawaited(sounds.play(SoundEvent.message, evenIfOff: true));
                  },
                ),
              ),
              const Icon(Icons.volume_up_rounded, size: 18, color: PColors.textMuted),
              SizedBox(
                width: 44,
                child: Text('${(sounds.volume * 100).round()}%', textAlign: TextAlign.right, style: PType.small),
              ),
            ],
          ),
          const SizedBox(height: 18),
          Text('SOUNDS', style: PType.label),
          const SizedBox(height: 6),
          for (final e in SoundEvent.values) _SoundRow(sounds: sounds, event: e),
        ],
      ),
    );
  }
}

class _PackRow extends StatelessWidget {
  const _PackRow({required this.sounds});

  final Sounds sounds;

  @override
  Widget build(BuildContext context) {
    final pack = sounds.pack;
    return Container(
      padding: const EdgeInsets.fromLTRB(12, 8, 8, 8),
      decoration: BoxDecoration(
        color: PColors.sunken,
        border: Border.all(color: PColors.border),
        borderRadius: BorderRadius.circular(12),
      ),
      child: Row(
        children: [
          const Icon(Icons.library_music_outlined, size: 20, color: PColors.accentText),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(pack == null ? 'Post\'s own' : pack.split('/').where((s) => s.isNotEmpty).last, style: const TextStyle(fontSize: 14, color: PColors.text)),
                if (pack != null) Text(pack, maxLines: 1, overflow: TextOverflow.ellipsis, style: PType.small),
              ],
            ),
          ),
          if (pack != null) TextButton(onPressed: () => unawaited(sounds.choosePack(null)), child: const Text('Use Post\'s own')),
          const SizedBox(width: 4),
          OutlinedButton(
            onPressed: () async {
              final folder = await getDirectoryPath(confirmButtonText: 'Use this pack');
              if (folder != null) await sounds.choosePack(folder);
            },
            child: const Text('Choose folder…'),
          ),
        ],
      ),
    );
  }
}

class _SoundRow extends StatelessWidget {
  const _SoundRow({required this.sounds, required this.event});

  final Sounds sounds;
  final SoundEvent event;

  @override
  Widget build(BuildContext context) {
    final on = !sounds.off.contains(event);
    final file = sounds.fileFor(event);
    final custom = sounds.custom.containsKey(event);
    final from = file == null ? 'Post\'s own' : '${custom ? 'Your file' : 'From the pack'}: ${file.split('/').last}';
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 4),
      child: Row(
        children: [
          SizedBox(
            width: 52,
            child: Switch(value: on, materialTapTargetSize: MaterialTapTargetSize.shrinkWrap, onChanged: (v) => unawaited(sounds.setOn(event, v))),
          ),
          const SizedBox(width: 14),
          Expanded(
            child: Opacity(
              opacity: on ? 1 : 0.5,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(event.title, style: const TextStyle(fontSize: 14, color: PColors.text)),
                  Text(from, maxLines: 1, overflow: TextOverflow.ellipsis, style: PType.small),
                ],
              ),
            ),
          ),
          PIconButton(
            icon: Icons.play_arrow_rounded,
            tooltip: 'Play ${event.title.toLowerCase()}',
            size: 32,
            iconSize: 18,
            onPressed: () => unawaited(sounds.play(event, evenIfOff: true)),
          ),
          PIconButton(
            icon: Icons.folder_open_outlined,
            tooltip: 'Choose a file',
            size: 32,
            iconSize: 16,
            onPressed: () async {
              const sounds_ = XTypeGroup(label: 'Sounds', extensions: soundExtensions);
              final picked = await openFile(acceptedTypeGroups: [sounds_]);
              if (picked != null) await sounds.chooseFile(event, picked.path);
            },
          ),
          if (custom)
            PIconButton(
              icon: Icons.restart_alt_rounded,
              tooltip: 'Back to the pack\'s',
              size: 32,
              iconSize: 16,
              onPressed: () => unawaited(sounds.chooseFile(event, null)),
            )
          else
            const SizedBox(width: 32),
        ],
      ),
    );
  }
}
