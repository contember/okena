import 'package:flutter/cupertino.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:provider/provider.dart';

import '../models/saved_server.dart';
import '../providers/connection_provider.dart';
import '../theme/app_theme.dart';

class ServerListScreen extends StatelessWidget {
  const ServerListScreen({super.key});

  @override
  Widget build(BuildContext context) {
    final provider = context.watch<ConnectionProvider>();

    return Scaffold(
      backgroundColor: OkenaColors.background,
      body: SafeArea(
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            // Header
            Padding(
              padding: const EdgeInsets.fromLTRB(24, 20, 24, 8),
              child: Row(
                mainAxisAlignment: MainAxisAlignment.spaceBetween,
                children: [
                  const Text('Servers', style: OkenaTypography.largeTitle),
                  GestureDetector(
                    onTap: () {
                      HapticFeedback.lightImpact();
                      _showAddServerSheet(context);
                    },
                    child: Container(
                      width: 36,
                      height: 36,
                      decoration: BoxDecoration(
                        color: OkenaColors.surfaceElevated,
                        shape: BoxShape.circle,
                        border: Border.all(color: OkenaColors.border, width: 0.5),
                      ),
                      child: const Icon(
                        CupertinoIcons.plus,
                        color: OkenaColors.accent,
                        size: 18,
                      ),
                    ),
                  ),
                ],
              ),
            ),
            // Content
            Expanded(
              child: provider.servers.isEmpty
                  ? _buildEmptyState(context)
                  : _buildServerList(context, provider),
            ),
          ],
        ),
      ),
    );
  }

  Widget _buildEmptyState(BuildContext context) {
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Container(
            width: 80,
            height: 80,
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              gradient: RadialGradient(
                colors: [
                  OkenaColors.accent.withOpacity(0.15),
                  OkenaColors.accent.withOpacity(0.0),
                ],
              ),
            ),
            child: Icon(
              Icons.terminal_rounded,
              size: 36,
              color: OkenaColors.accent.withOpacity(0.8),
            ),
          ),
          const SizedBox(height: 20),
          const Text('No servers yet', style: OkenaTypography.title),
          const SizedBox(height: 8),
          Text(
            'Add a server to get started',
            style: OkenaTypography.body.copyWith(color: OkenaColors.textSecondary),
          ),
          const SizedBox(height: 28),
          SizedBox(
            width: 180,
            child: FilledButton(
              onPressed: () => _showAddServerSheet(context),
              child: const Text('Add Server'),
            ),
          ),
        ],
      ),
    );
  }

  Widget _buildServerList(BuildContext context, ConnectionProvider provider) {
    return ListView.builder(
      padding: const EdgeInsets.fromLTRB(16, 8, 16, 24),
      itemCount: provider.servers.length,
      itemBuilder: (context, index) {
        final server = provider.servers[index];
        return Padding(
          padding: const EdgeInsets.only(bottom: 8),
          child: Dismissible(
            key: ValueKey('${server.host}:${server.port}'),
            direction: DismissDirection.endToStart,
            background: Container(
              alignment: Alignment.centerRight,
              padding: const EdgeInsets.only(right: 24),
              decoration: BoxDecoration(
                color: OkenaColors.error.withOpacity(0.15),
                borderRadius: BorderRadius.circular(14),
              ),
              child: const Icon(CupertinoIcons.delete, color: OkenaColors.error, size: 20),
            ),
            onDismissed: (_) => provider.removeServer(server),
            child: GestureDetector(
              onTap: () {
                HapticFeedback.selectionClick();
                provider.connectTo(server);
              },
              child: Container(
                padding: const EdgeInsets.all(16),
                decoration: BoxDecoration(
                  color: OkenaColors.surface,
                  borderRadius: BorderRadius.circular(14),
                  border: Border.all(color: OkenaColors.border, width: 0.5),
                ),
                child: Row(
                  children: [
                    // Letter avatar
                    Container(
                      width: 40,
                      height: 40,
                      decoration: BoxDecoration(
                        color: OkenaColors.accent.withOpacity(0.12),
                        borderRadius: BorderRadius.circular(10),
                      ),
                      alignment: Alignment.center,
                      child: Text(
                        server.displayName[0].toUpperCase(),
                        style: OkenaTypography.headline.copyWith(
                          color: OkenaColors.accent,
                        ),
                      ),
                    ),
                    const SizedBox(width: 14),
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(
                            server.displayName,
                            style: OkenaTypography.body.copyWith(
                              fontWeight: FontWeight.w500,
                            ),
                          ),
                          if (server.label != null) ...[
                            const SizedBox(height: 3),
                            Text(
                              '${server.host}:${server.port}',
                              style: OkenaTypography.caption.copyWith(
                                fontFamily: 'JetBrainsMono',
                                color: OkenaColors.textTertiary,
                              ),
                            ),
                          ],
                        ],
                      ),
                    ),
                    const Icon(
                      CupertinoIcons.chevron_right,
                      color: OkenaColors.textTertiary,
                      size: 16,
                    ),
                  ],
                ),
              ),
            ),
          ),
        );
      },
    );
  }

  void _showAddServerSheet(BuildContext context) {
    final hostController = TextEditingController();
    final portController = TextEditingController(text: '19100');
    final provider = context.read<ConnectionProvider>();

    showModalBottomSheet(
      context: context,
      isScrollControlled: true,
      backgroundColor: OkenaColors.surface,
      shape: const RoundedRectangleBorder(
        borderRadius: BorderRadius.vertical(top: Radius.circular(16)),
      ),
      builder: (ctx) => Padding(
        padding: EdgeInsets.only(
          left: 24,
          right: 24,
          top: 8,
          bottom: MediaQuery.of(ctx).viewInsets.bottom + 24,
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            // Drag handle
            Center(
              child: Container(
                width: 36,
                height: 4,
                margin: const EdgeInsets.only(bottom: 20),
                decoration: BoxDecoration(
                  color: OkenaColors.textTertiary.withOpacity(0.4),
                  borderRadius: BorderRadius.circular(2),
                ),
              ),
            ),
            const Text('Add Server', style: OkenaTypography.title),
            const SizedBox(height: 4),
            Text(
              'Enter the host and port of your Okena desktop app',
              style: OkenaTypography.callout.copyWith(color: OkenaColors.textTertiary),
            ),
            const SizedBox(height: 24),
            TextField(
              controller: hostController,
              decoration: const InputDecoration(
                labelText: 'Host',
                hintText: '192.168.1.100',
              ),
              autofocus: true,
              keyboardType: TextInputType.url,
              style: OkenaTypography.body,
            ),
            const SizedBox(height: 14),
            TextField(
              controller: portController,
              decoration: const InputDecoration(
                labelText: 'Port',
              ),
              keyboardType: TextInputType.number,
              style: OkenaTypography.body,
            ),
            const SizedBox(height: 24),
            FilledButton(
              onPressed: () {
                final host = hostController.text.trim();
                final port =
                    int.tryParse(portController.text.trim()) ?? 19100;
                if (host.isNotEmpty) {
                  HapticFeedback.mediumImpact();
                  provider.addServer(SavedServer(host: host, port: port));
                  Navigator.of(ctx).pop();
                }
              },
              child: const Text('Add Server'),
            ),
          ],
        ),
      ),
    );
  }
}
