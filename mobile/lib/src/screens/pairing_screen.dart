import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../providers/connection_provider.dart';
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
      appBar: AppBar(
        leading: IconButton(
          icon: const Icon(Icons.arrow_back),
          onPressed: () => provider.disconnect(),
        ),
        title: Text(provider.activeServer?.displayName ?? 'Connecting'),
      ),
      body: Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          mainAxisAlignment: MainAxisAlignment.center,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Center(
              child: StatusIndicator(status: provider.status),
            ),
            const SizedBox(height: 32),
            if (!showCodeInput && !isError) ...[
              const Center(child: CircularProgressIndicator()),
              const SizedBox(height: 16),
              Text(
                'Connecting to server...',
                textAlign: TextAlign.center,
                style: Theme.of(context).textTheme.bodyLarge,
              ),
            ],
            if (showCodeInput) ...[
              Text(
                'Enter Pairing Code',
                textAlign: TextAlign.center,
                style: Theme.of(context).textTheme.titleLarge,
              ),
              const SizedBox(height: 8),
              Text(
                'Check the Okena desktop app for the pairing code.',
                textAlign: TextAlign.center,
                style: Theme.of(context).textTheme.bodyMedium?.copyWith(
                      color: Colors.grey,
                    ),
              ),
              const SizedBox(height: 24),
              TextField(
                controller: _codeController,
                decoration: const InputDecoration(
                  labelText: 'XXXX-XXXX',
                  border: OutlineInputBorder(),
                ),
                textAlign: TextAlign.center,
                style: const TextStyle(
                  fontSize: 24,
                  letterSpacing: 4,
                  fontFamily: 'monospace',
                ),
                textCapitalization: TextCapitalization.characters,
                keyboardType: TextInputType.text,
                autofocus: true,
                onSubmitted: (_) => _submitCode(),
              ),
              const SizedBox(height: 16),
              FilledButton(
                onPressed: _submitting ? null : _submitCode,
                child: _submitting
                    ? const SizedBox(
                        height: 20,
                        width: 20,
                        child: CircularProgressIndicator(strokeWidth: 2),
                      )
                    : const Text('Pair'),
              ),
            ],
            if (isError) ...[
              Icon(Icons.error_outline, size: 48, color: Colors.red.shade300),
              const SizedBox(height: 16),
              Text(
                errorMessage ?? 'Connection failed',
                textAlign: TextAlign.center,
                style: TextStyle(color: Colors.red.shade300),
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
                child: const Text('Retry'),
              ),
            ],
          ],
        ),
      ),
    );
  }
}
