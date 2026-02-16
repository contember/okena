import 'package:flutter/material.dart';

import '../../src/rust/api/connection.dart';
import '../theme/app_theme.dart';

class StatusIndicator extends StatefulWidget {
  final ConnectionStatus status;

  const StatusIndicator({super.key, required this.status});

  @override
  State<StatusIndicator> createState() => _StatusIndicatorState();
}

class _StatusIndicatorState extends State<StatusIndicator>
    with SingleTickerProviderStateMixin {
  late AnimationController _pulseController;
  late Animation<double> _pulseAnimation;

  @override
  void initState() {
    super.initState();
    _pulseController = AnimationController(
      vsync: this,
      duration: const Duration(milliseconds: 1200),
    );
    _pulseAnimation = Tween<double>(begin: 0.4, end: 1.0).animate(
      CurvedAnimation(parent: _pulseController, curve: Curves.easeInOut),
    );
    _updatePulse();
  }

  @override
  void didUpdateWidget(StatusIndicator oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.status.runtimeType != widget.status.runtimeType) {
      _updatePulse();
    }
  }

  void _updatePulse() {
    final shouldPulse = widget.status is ConnectionStatus_Connecting ||
        widget.status is ConnectionStatus_Pairing;
    if (shouldPulse) {
      _pulseController.repeat(reverse: true);
    } else {
      _pulseController.stop();
      _pulseController.value = 1.0;
    }
  }

  @override
  void dispose() {
    _pulseController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final (color, label) = switch (widget.status) {
      ConnectionStatus_Disconnected() => (OkenaColors.textTertiary, 'Disconnected'),
      ConnectionStatus_Connecting() => (OkenaColors.warning, 'Connecting'),
      ConnectionStatus_Connected() => (OkenaColors.success, 'Connected'),
      ConnectionStatus_Pairing() => (OkenaColors.accent, 'Pairing'),
      ConnectionStatus_Error(:final message) => (OkenaColors.error, 'Error: $message'),
    };

    final isConnected = widget.status is ConnectionStatus_Connected;

    return AnimatedBuilder(
      animation: _pulseAnimation,
      builder: (context, child) {
        return Container(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
          decoration: BoxDecoration(
            color: color.withOpacity(0.1 * _pulseAnimation.value),
            borderRadius: BorderRadius.circular(20),
            border: Border.all(
              color: color.withOpacity(0.2 * _pulseAnimation.value),
              width: 0.5,
            ),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Container(
                width: 6,
                height: 6,
                decoration: BoxDecoration(
                  color: color.withOpacity(_pulseAnimation.value),
                  shape: BoxShape.circle,
                  boxShadow: isConnected
                      ? [
                          BoxShadow(
                            color: color.withOpacity(0.5),
                            blurRadius: 6,
                            spreadRadius: 1,
                          ),
                        ]
                      : null,
                ),
              ),
              const SizedBox(width: 6),
              Flexible(
                child: Text(
                  label,
                  style: OkenaTypography.caption2.copyWith(
                    color: color.withOpacity(_pulseAnimation.value),
                    fontWeight: FontWeight.w500,
                  ),
                  overflow: TextOverflow.ellipsis,
                ),
              ),
            ],
          ),
        );
      },
    );
  }
}
