import 'package:flutter/material.dart';
import 'package:mobile/src/rust/api/connection.dart';
import 'package:mobile/src/rust/frb_generated.dart';

Future<void> main() async {
  await RustLib.init();
  runApp(const OkenaApp());
}

class OkenaApp extends StatelessWidget {
  const OkenaApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Okena',
      theme: ThemeData.dark(useMaterial3: true).copyWith(
        colorScheme: ColorScheme.dark(
          primary: Colors.blue.shade300,
          surface: const Color(0xFF1E1E2E),
        ),
        scaffoldBackgroundColor: const Color(0xFF1E1E2E),
      ),
      home: const ConnectionScreen(),
    );
  }
}

class ConnectionScreen extends StatefulWidget {
  const ConnectionScreen({super.key});

  @override
  State<ConnectionScreen> createState() => _ConnectionScreenState();
}

class _ConnectionScreenState extends State<ConnectionScreen> {
  final _hostController = TextEditingController(text: '192.168.1.100');
  final _portController = TextEditingController(text: '19100');
  String _status = 'Disconnected';
  String? _connId;

  @override
  void dispose() {
    _hostController.dispose();
    _portController.dispose();
    super.dispose();
  }

  void _connect() {
    final host = _hostController.text.trim();
    final port = int.tryParse(_portController.text.trim()) ?? 19100;

    setState(() {
      _connId = connect(host: host, port: port);
      _status = 'Connected (stub): $_connId';
    });
  }

  void _disconnect() {
    if (_connId != null) {
      disconnect(connId: _connId!);
      setState(() {
        _connId = null;
        _status = 'Disconnected';
      });
    }
  }

  void _checkStatus() {
    if (_connId != null) {
      final status = connectionStatus(connId: _connId!);
      setState(() {
        _status = switch (status) {
          ConnectionStatus_Disconnected() => 'Disconnected',
          ConnectionStatus_Connecting() => 'Connecting...',
          ConnectionStatus_Connected() => 'Connected',
          ConnectionStatus_Pairing() => 'Pairing...',
          ConnectionStatus_Error(:final message) => 'Error: $message',
        };
      });
    }
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: const Text('Okena Remote'),
        centerTitle: true,
      ),
      body: Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            const SizedBox(height: 32),
            Icon(
              Icons.terminal,
              size: 64,
              color: Theme.of(context).colorScheme.primary,
            ),
            const SizedBox(height: 16),
            Text(
              'Okena',
              style: Theme.of(context).textTheme.headlineLarge,
              textAlign: TextAlign.center,
            ),
            const SizedBox(height: 8),
            Text(
              'Remote Terminal',
              style: Theme.of(context).textTheme.bodyLarge?.copyWith(
                    color: Colors.grey,
                  ),
              textAlign: TextAlign.center,
            ),
            const SizedBox(height: 48),
            TextField(
              controller: _hostController,
              decoration: const InputDecoration(
                labelText: 'Host',
                border: OutlineInputBorder(),
                prefixIcon: Icon(Icons.dns),
              ),
            ),
            const SizedBox(height: 16),
            TextField(
              controller: _portController,
              decoration: const InputDecoration(
                labelText: 'Port',
                border: OutlineInputBorder(),
                prefixIcon: Icon(Icons.numbers),
              ),
              keyboardType: TextInputType.number,
            ),
            const SizedBox(height: 24),
            FilledButton.icon(
              onPressed: _connId == null ? _connect : _disconnect,
              icon: Icon(_connId == null ? Icons.link : Icons.link_off),
              label: Text(_connId == null ? 'Connect' : 'Disconnect'),
            ),
            if (_connId != null) ...[
              const SizedBox(height: 8),
              OutlinedButton.icon(
                onPressed: _checkStatus,
                icon: const Icon(Icons.refresh),
                label: const Text('Check Status'),
              ),
            ],
            const SizedBox(height: 24),
            Container(
              padding: const EdgeInsets.all(16),
              decoration: BoxDecoration(
                color: Colors.black26,
                borderRadius: BorderRadius.circular(8),
              ),
              child: Text(
                'Status: $_status',
                style: const TextStyle(fontFamily: 'monospace'),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
