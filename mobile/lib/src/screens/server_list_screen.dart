import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import '../models/saved_server.dart';
import '../providers/connection_provider.dart';

class ServerListScreen extends StatelessWidget {
  const ServerListScreen({super.key});

  @override
  Widget build(BuildContext context) {
    final provider = context.watch<ConnectionProvider>();

    return Scaffold(
      appBar: AppBar(
        title: const Text('Okena'),
        centerTitle: true,
      ),
      body: provider.servers.isEmpty
          ? Center(
              child: Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  Icon(
                    Icons.terminal,
                    size: 64,
                    color: Theme.of(context).colorScheme.primary,
                  ),
                  const SizedBox(height: 16),
                  Text(
                    'No servers yet',
                    style: Theme.of(context).textTheme.titleMedium,
                  ),
                  const SizedBox(height: 8),
                  Text(
                    'Add a server to get started',
                    style: Theme.of(context).textTheme.bodyMedium?.copyWith(
                          color: Colors.grey,
                        ),
                  ),
                ],
              ),
            )
          : ListView.builder(
              padding: const EdgeInsets.symmetric(vertical: 8),
              itemCount: provider.servers.length,
              itemBuilder: (context, index) {
                final server = provider.servers[index];
                return Dismissible(
                  key: ValueKey('${server.host}:${server.port}'),
                  direction: DismissDirection.endToStart,
                  background: Container(
                    alignment: Alignment.centerRight,
                    padding: const EdgeInsets.only(right: 24),
                    color: Colors.red,
                    child: const Icon(Icons.delete, color: Colors.white),
                  ),
                  onDismissed: (_) => provider.removeServer(server),
                  child: ListTile(
                    leading: const Icon(Icons.dns),
                    title: Text(server.displayName),
                    subtitle: server.label != null
                        ? Text('${server.host}:${server.port}')
                        : null,
                    trailing: const Icon(Icons.chevron_right),
                    onTap: () => provider.connectTo(server),
                  ),
                );
              },
            ),
      floatingActionButton: FloatingActionButton(
        onPressed: () => _showAddServerSheet(context),
        child: const Icon(Icons.add),
      ),
    );
  }

  void _showAddServerSheet(BuildContext context) {
    final hostController = TextEditingController();
    final portController = TextEditingController(text: '19100');
    final provider = context.read<ConnectionProvider>();

    showModalBottomSheet(
      context: context,
      isScrollControlled: true,
      builder: (ctx) => Padding(
        padding: EdgeInsets.only(
          left: 24,
          right: 24,
          top: 24,
          bottom: MediaQuery.of(ctx).viewInsets.bottom + 24,
        ),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text(
              'Add Server',
              style: Theme.of(ctx).textTheme.titleLarge,
            ),
            const SizedBox(height: 16),
            TextField(
              controller: hostController,
              decoration: const InputDecoration(
                labelText: 'Host',
                border: OutlineInputBorder(),
                hintText: '192.168.1.100',
              ),
              autofocus: true,
              keyboardType: TextInputType.url,
            ),
            const SizedBox(height: 12),
            TextField(
              controller: portController,
              decoration: const InputDecoration(
                labelText: 'Port',
                border: OutlineInputBorder(),
              ),
              keyboardType: TextInputType.number,
            ),
            const SizedBox(height: 16),
            FilledButton(
              onPressed: () {
                final host = hostController.text.trim();
                final port =
                    int.tryParse(portController.text.trim()) ?? 19100;
                if (host.isNotEmpty) {
                  provider.addServer(SavedServer(host: host, port: port));
                  Navigator.of(ctx).pop();
                }
              },
              child: const Text('Add'),
            ),
          ],
        ),
      ),
    );
  }
}
