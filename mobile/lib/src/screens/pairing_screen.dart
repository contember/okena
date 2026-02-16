import 'package:flutter/cupertino.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';

import '../providers/connection_provider.dart';
import '../theme/app_theme.dart';
import '../widgets/status_indicator.dart';
import '../../src/rust/api/connection.dart';

class PairingScreen extends StatefulWidget {
  const PairingScreen({super.key});

  @override
  State<PairingScreen> createState() => _PairingScreenState();
}

class _PairingScreenState extends State<PairingScreen> {
  final _codeController = TextEditingController();
  bool _submitting = false;

  @override
  void dispose() {
    _codeController.dispose();
    super.dispose();
  }

  Future<void> _submitCode() async {
    final code = _codeController.text.trim();
    if (code.isEmpty) return;

    HapticFeedback.mediumImpact();
    setState(() => _submitting = true);
    final provider = context.read<ConnectionProvider>();
    await provider.pair(code);
    if (mounted) {
      setState(() => _submitting = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    final provider = context.watch<ConnectionProvider>();
    final showCodeInput = provider.isPairing;
    final isError = provider.status is ConnectionStatus_Error;
    final errorMessage = isError
        ? (provider.status as ConnectionStatus_Error).message
        : null;

    return Scaffold(
      backgroundColor: OkenaColors.background,
      body: SafeArea(
        child: Column(
          children: [
            // Header
            Padding(
              padding: const EdgeInsets.fromLTRB(4, 4, 16, 0),
              child: Row(
                children: [
                  IconButton(
                    icon: const Icon(
                      CupertinoIcons.chevron_back,
                      color: OkenaColors.accent,
                      size: 22,
                    ),
                    onPressed: () => provider.disconnect(),
                  ),
                  const SizedBox(width: 4),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(
                          provider.activeServer?.displayName ?? 'Server',
                          style: OkenaTypography.headline,
                        ),
                        const SizedBox(height: 1),
                        Text(
                          'Connecting',
                          style: OkenaTypography.caption2.copyWith(
                            color: OkenaColors.textTertiary,
                          ),
                        ),
                      ],
                    ),
                  ),
                ],
              ),
            ),
            // Body
            Expanded(
              child: Padding(
                padding: const EdgeInsets.all(24),
                child: Column(
                  mainAxisAlignment: MainAxisAlignment.center,
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    Center(
                      child: StatusIndicator(status: provider.status),
                    ),
                    const SizedBox(height: 40),
                    if (!showCodeInput && !isError) ...[
                      const Center(
                        child: CupertinoActivityIndicator(
                          radius: 14,
                          color: OkenaColors.textSecondary,
                        ),
                      ),
                      const SizedBox(height: 20),
                      Text(
                        'Connecting to server...',
                        textAlign: TextAlign.center,
                        style: OkenaTypography.body.copyWith(
                          color: OkenaColors.textSecondary,
                        ),
                      ),
                    ],
                    if (showCodeInput) ...[
                      Text(
                        'Pair with Server',
                        textAlign: TextAlign.center,
                        style: OkenaTypography.largeTitle,
                      ),
                      const SizedBox(height: 8),
                      Text(
                        'Check the Okena desktop app for the pairing code.',
                        textAlign: TextAlign.center,
                        style: OkenaTypography.body.copyWith(
                          color: OkenaColors.textSecondary,
                        ),
                      ),
                      const SizedBox(height: 32),
                      TextField(
                        controller: _codeController,
                        decoration: const InputDecoration(
                          hintText: 'XXXX-XXXX',
                        ),
                        textAlign: TextAlign.center,
                        style: const TextStyle(
                          fontSize: 28,
                          letterSpacing: 8,
                          fontFamily: 'JetBrainsMono',
                          fontWeight: FontWeight.w500,
                          color: OkenaColors.textPrimary,
                        ),
                        textCapitalization: TextCapitalization.characters,
                        keyboardType: TextInputType.text,
                        autofocus: true,
                        onSubmitted: (_) => _submitCode(),
                      ),
                      const SizedBox(height: 24),
                      FilledButton(
                        onPressed: _submitting ? null : _submitCode,
                        child: _submitting
                            ? const CupertinoActivityIndicator(
                                radius: 10,
                                color: Colors.white,
                              )
                            : const Text('Pair'),
                      ),
                    ],
                    if (isError) ...[
                      Center(
                        child: Container(
                          width: 64,
                          height: 64,
                          decoration: BoxDecoration(
                            color: OkenaColors.error.withOpacity(0.1),
                            shape: BoxShape.circle,
                          ),
                          child: const Icon(
                            CupertinoIcons.xmark_circle,
                            size: 32,
                            color: OkenaColors.error,
                          ),
                        ),
                      ),
                      const SizedBox(height: 20),
                      Container(
                        padding: const EdgeInsets.all(16),
                        decoration: BoxDecoration(
                          color: OkenaColors.error.withOpacity(0.08),
                          borderRadius: BorderRadius.circular(12),
                          border: Border.all(
                            color: OkenaColors.error.withOpacity(0.2),
                            width: 0.5,
                          ),
                        ),
                        child: Text(
                          errorMessage ?? 'Connection failed',
                          textAlign: TextAlign.center,
                          style: OkenaTypography.body.copyWith(
                            color: OkenaColors.error,
                          ),
                        ),
                      ),
                      const SizedBox(height: 24),
                      OutlinedButton(
                        onPressed: () {
                          final server = provider.activeServer;
                          if (server != null) {
                            provider.disconnect();
                            provider.connectTo(server);
                          }
                        },
                        child: const Text('Try Again'),
                      ),
                    ],
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
