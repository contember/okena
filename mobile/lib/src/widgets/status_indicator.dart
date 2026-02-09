import 'package:flutter/material.dart';

import '../../src/rust/api/connection.dart';

class StatusIndicator extends StatelessWidget {
  final ConnectionStatus status;

  const StatusIndicator({super.key, required this.status});

  @override
  Widget build(BuildContext context) {
    final (color, label) = switch (status) {
      ConnectionStatus_Disconnected() => (Colors.grey, 'Disconnected'),
      ConnectionStatus_Connecting() => (Colors.orange, 'Connecting'),
      ConnectionStatus_Connected() => (Colors.green, 'Connected'),
      ConnectionStatus_Pairing() => (Colors.blue, 'Pairing'),
      ConnectionStatus_Error(:final message) => (Colors.red, 'Error: $message'),
    };

    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Container(
          width: 8,
          height: 8,
          decoration: BoxDecoration(
            color: color,
            shape: BoxShape.circle,
          ),
        ),
        const SizedBox(width: 6),
        Flexible(
          child: Text(
            label,
            style: TextStyle(color: color, fontSize: 12),
            overflow: TextOverflow.ellipsis,
          ),
        ),
      ],
    );
  }
}
