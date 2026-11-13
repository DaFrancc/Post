// App updates as the user sees them (design section 12, "App updates"):
// a card above your name once an update is downloading or ready, a banner
// when this version is too old for the server, the Updates page in
// Settings, and restarting into the new version.

import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';

import '../state/app_state.dart';
import '../theme.dart';
import '../ui/controls.dart';
import '../ui/format.dart';
import '../ui/window_frame.dart';
import '../version.dart';

/// Install the downloaded update and start the new version in place of
/// this one.
Future<void> restartIntoUpdate(BuildContext context, AppState state) async {
  final program = await state.installUpdate();
  if (program == null) {
    if (context.mounted) showToast(context, state.updateError ?? 'The update could not be installed.', error: true);
    return;
  }
  // The new version starts once this one has gone, so the two never hold
  // the profile at once.
  await Process.start('/bin/sh', ['-c', r'while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; exec "$0"', program, '$pid'], mode: ProcessStartMode.detached);
  await closeWindow();
}

/// Update now, whatever has happened so far: download the update if it has
/// not been (automatic updates off, or a download that failed), then install
/// it and restart into it.
Future<void> updateNow(BuildContext context, AppState state) async {
  if (state.updateStage == UpdateStage.idle) await state.downloadUpdate();
  if (state.updateStage == UpdateStage.ready && context.mounted) {
    await restartIntoUpdate(context, state);
  } else if (state.updateError != null && context.mounted) {
    showToast(context, state.updateError!, error: true);
  }
}

/// The green Update button, on the card and the Updates page. It shows
/// how far a download or an install has got while it works.
class UpdateButton extends StatelessWidget {
  const UpdateButton({super.key, required this.state, this.label = 'Update', this.compact = false});

  final AppState state;
  final String label;

  /// Small, for the card.
  final bool compact;

  @override
  Widget build(BuildContext context) {
    final stage = state.updateStage;
    final busy = stage == UpdateStage.checking || stage == UpdateStage.downloading || stage == UpdateStage.installing;
    final text = switch (stage) {
      UpdateStage.downloading => '${(state.updateProgress * 100).floor()}%',
      UpdateStage.installing => 'Installing…',
      _ => label,
    };
    return FilledButton.icon(
      onPressed: busy ? null : () => unawaited(updateNow(context, state)),
      // Green, and readable while it shows its progress.
      style: PButtons.tactile(
        face: PColors.update,
        onFace: PColors.onUpdate,
        fadeDisabled: false,
      ).merge(FilledButton.styleFrom(minimumSize: compact ? const Size(0, 32) : null, padding: compact ? const EdgeInsets.symmetric(horizontal: 12) : null)),
      icon: busy ? const PSpinner(size: 14, color: PColors.onUpdate) : Icon(Icons.system_update_alt_rounded, size: compact ? 16 : 18),
      label: Text(text),
    );
  }
}

/// While the app runs: a downloaded update installs as the window closes,
/// so the next start is the new version.
void installUpdatesOnClose(AppState state) {
  beforeWindowClose = () async {
    if (state.updateStage == UpdateStage.ready) await state.installUpdate();
  };
}

String _size(BigInt bytes) {
  final mb = bytes.toDouble() / (1024 * 1024);
  return mb >= 10 ? '${mb.round()} MB' : '${mb.toStringAsFixed(1)} MB';
}

// ---------------------------------------------------------------- the card

/// Above your name: an update on its way, ready, or out but needing to be
/// installed by hand. Nothing otherwise.
class UpdateCard extends StatelessWidget {
  const UpdateCard({super.key, required this.state, required this.onDetails});

  final AppState state;

  /// Open the Updates page in Settings.
  final VoidCallback onDetails;

  @override
  Widget build(BuildContext context) {
    final u = state.update;
    final stage = state.updateStage;
    final Widget? card = switch ((u, stage)) {
      (null, _) => null,
      (final u?, UpdateStage.downloading) => _card(
        key: 'downloading',
        title: 'Downloading Post ${u.version}',
        detail: '${(state.updateProgress * 100).floor()}% of ${_size(u.size)}',
        progress: state.updateProgress,
        onTap: onDetails,
      ),
      (final u?, UpdateStage.ready || UpdateStage.installing) => _card(
        key: 'ready',
        title: 'Post ${u.version} is ready',
        detail: stage == UpdateStage.installing ? 'Installing…' : 'Update restarts Post into it.',
        action: UpdateButton(state: state, compact: true),
        onTap: onDetails,
      ),
      // Built from source: it never installs itself, so it only says so.
      (final u?, _) when !u.canInstall => _card(
        key: 'manual',
        title: 'Post ${u.version} is out',
        detail: 'Built from source: see how to update',
        onTap: onDetails,
      ),
      // Not downloaded yet: automatic updates are off, or a download failed.
      (final u?, _) => _card(
        key: 'available',
        title: 'Post ${u.version} is out',
        detail: 'Update downloads it and restarts.',
        action: UpdateButton(state: state, compact: true),
        onTap: onDetails,
      ),
    };
    return AnimatedSize(
      duration: PMotion.normal,
      curve: PMotion.curve,
      alignment: Alignment.bottomCenter,
      child: AnimatedSwitcher(
        duration: PMotion.normal,
        child: card ?? const SizedBox(key: ValueKey('none'), width: double.infinity),
      ),
    );
  }

  Widget _card({required String key, required String title, required String detail, double? progress, Widget? action, required VoidCallback onTap}) {
    return Padding(
      key: ValueKey(key),
      padding: const EdgeInsets.fromLTRB(8, 0, 8, 8),
      child: Material(
        color: PColors.accentSoft,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(14),
          side: const BorderSide(color: PColors.activeEdge),
        ),
        clipBehavior: Clip.antiAlias,
        child: InkWell(
          onTap: onTap,
          mouseCursor: SystemMouseCursors.click,
          child: Padding(
            padding: const EdgeInsets.fromLTRB(12, 10, 10, 10),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Row(
                  children: [
                    const Icon(Icons.system_update_alt_rounded, size: 18, color: PColors.accentText),
                    const SizedBox(width: 10),
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(
                            title,
                            style: const TextStyle(fontSize: 13, fontWeight: FontWeight.w600, color: PColors.textStrong),
                          ),
                          Text(detail, style: const TextStyle(fontSize: 11.5, color: PColors.textMuted)),
                        ],
                      ),
                    ),
                    if (action != null) ...[const SizedBox(width: 8), action],
                  ],
                ),
                if (progress != null) ...[
                  const SizedBox(height: 8),
                  ClipRRect(
                    borderRadius: BorderRadius.circular(2),
                    child: LinearProgressIndicator(value: progress, minHeight: 3, backgroundColor: PColors.selected, color: PColors.accent),
                  ),
                ],
              ],
            ),
          ),
        ),
      ),
    );
  }
}

// ---------------------------------------------------------------- too old

/// Across the top when this version is older than the newest release's
/// minimum: it can no longer keep talking to the server once that is
/// running.
class UpdateRequiredBanner extends StatelessWidget {
  const UpdateRequiredBanner({super.key, required this.state, required this.onDetails});

  final AppState state;
  final VoidCallback onDetails;

  @override
  Widget build(BuildContext context) {
    final u = state.update;
    final show = u != null && u.mandatory;
    return AnimatedSize(
      duration: PMotion.normal,
      curve: PMotion.curve,
      alignment: Alignment.topCenter,
      child: !show
          ? const SizedBox(width: double.infinity)
          : Container(
              margin: const EdgeInsets.fromLTRB(PShape.gap, 0, PShape.gap, PShape.gap),
              padding: const EdgeInsets.fromLTRB(14, 8, 8, 8),
              decoration: BoxDecoration(
                color: PColors.dangerSoft,
                border: Border.all(color: PColors.danger.withValues(alpha: 0.5)),
                borderRadius: BorderRadius.circular(12),
              ),
              child: Row(
                children: [
                  const Icon(Icons.warning_amber_rounded, size: 18, color: PColors.dangerText),
                  const SizedBox(width: 10),
                  Expanded(
                    child: Text(
                      'This version of Post is too old for your server. Update to Post ${u.version} to keep messaging.',
                      style: const TextStyle(fontSize: 13, color: PColors.dangerText),
                    ),
                  ),
                  const SizedBox(width: 10),
                  if (u.canInstall) UpdateButton(state: state) else OutlinedButton(onPressed: onDetails, child: const Text('Details')),
                ],
              ),
            ),
    );
  }
}

// ---------------------------------------------------------------- settings

/// The Updates page: this version, what is new, and the controls.
class UpdateSettings extends StatelessWidget {
  const UpdateSettings({super.key, required this.state, required this.checkRow});

  final AppState state;

  /// The settings page's checkbox row, for the automatic setting.
  final Widget Function({required bool value, required String label, String? help, required ValueChanged<bool> onChanged}) checkRow;

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: state,
      builder: (context, _) {
        final u = state.update;
        final stage = state.updateStage;
        final checked = state.updateCheckedAt;
        final busy = stage == UpdateStage.checking || stage == UpdateStage.downloading || stage == UpdateStage.installing;
        return ListView(
          padding: const EdgeInsets.fromLTRB(28, 16, 28, 28),
          children: [
            Text('POST', style: PType.label),
            const SizedBox(height: 6),
            Text(isDevelopmentBuild ? 'Development build' : 'Version $appVersion', style: const TextStyle(fontSize: 15, color: PColors.text)),
            const SizedBox(height: 4),
            Text(switch ((stage, u, checked)) {
              (UpdateStage.checking, _, _) => 'Checking with your server…',
              (_, null, final at?) => 'Up to date. Checked ${messageTime(at.millisecondsSinceEpoch)}.',
              (_, null, null) => 'Updates come from your server, signed by Post\'s release key.',
              (_, final u?, _) => 'Post ${u.version} is available (${_size(u.size)}).',
            }, style: PType.small),
            const SizedBox(height: 14),
            Wrap(
              spacing: 10,
              runSpacing: 10,
              children: [
                if (u != null && u.canInstall) UpdateButton(state: state, label: 'Update to ${u.version}'),
                OutlinedButton.icon(
                  onPressed: busy || stage == UpdateStage.ready ? null : () => unawaited(state.checkForUpdate()),
                  icon: stage == UpdateStage.checking ? const PSpinner(size: 16) : const Icon(Icons.refresh_rounded, size: 18),
                  label: const Text('Check for updates'),
                ),
              ],
            ),
            if (stage == UpdateStage.downloading) ...[
              const SizedBox(height: 10),
              ClipRRect(
                borderRadius: BorderRadius.circular(2),
                child: LinearProgressIndicator(value: state.updateProgress, minHeight: 4, backgroundColor: PColors.selected, color: PColors.accent),
              ),
            ],
            if (state.updateError != null) ...[const SizedBox(height: 12), PErrorBox(state.updateError!)],
            if (u != null) ...[
              const SizedBox(height: 22),
              Text('WHAT\'S NEW IN ${u.version}', style: PType.label),
              const SizedBox(height: 8),
              Container(
                padding: const EdgeInsets.all(12),
                decoration: BoxDecoration(
                  color: PColors.sunken,
                  border: Border.all(color: PColors.border),
                  borderRadius: BorderRadius.circular(12),
                ),
                child: SelectableText(
                  u.notes.isEmpty ? 'No notes for this release.' : u.notes,
                  style: const TextStyle(fontSize: 13.5, height: 1.5, color: PColors.textBody),
                ),
              ),
              if (!u.canInstall) ...[
                const SizedBox(height: 12),
                const Text(
                  'This copy of Post was built from source, so it does not install updates or add itself to your apps. '
                  'Pull the newest code and build it again. Copies installed from a release update themselves.',
                  style: PType.small,
                ),
              ],
            ],
            const SizedBox(height: 24),
            checkRow(
              value: state.autoUpdate,
              label: 'Check for updates automatically',
              help:
                  'Every few hours, your server is asked for a newer release, which then downloads in the background. '
                  'Nothing installs until you restart or quit Post.',
              onChanged: (v) => unawaited(state.setAutoUpdate(v)),
            ),
          ],
        );
      },
    );
  }
}
