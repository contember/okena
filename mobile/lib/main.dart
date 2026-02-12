import 'package:flutter/material.dart';
import 'package:provider/provider.dart';

import 'src/providers/connection_provider.dart';
import 'src/providers/workspace_provider.dart';
import 'src/screens/server_list_screen.dart';
import 'src/screens/pairing_screen.dart';
import 'src/screens/workspace_screen.dart';
import 'src/rust/frb_generated.dart';

Future<void> main() async {
  await RustLib.init();
  runApp(const OkenaApp());
}

class OkenaApp extends StatelessWidget {
  const OkenaApp({super.key});

  @override
  Widget build(BuildContext context) {
    return MultiProvider(
      providers: [
        ChangeNotifierProvider(create: (_) => ConnectionProvider()),
        ChangeNotifierProxyProvider<ConnectionProvider, WorkspaceProvider>(
          create: (ctx) =>
              WorkspaceProvider(ctx.read<ConnectionProvider>()),
          update: (_, connection, previous) =>
              previous ?? WorkspaceProvider(connection),
        ),
      ],
      child: MaterialApp(
        title: 'Okena',
        theme: ThemeData.dark(useMaterial3: true).copyWith(
          colorScheme: const ColorScheme.dark(
            primary: Color(0xFF007ACC),
            surface: Color(0xFF1E1E1E),
          ),
          scaffoldBackgroundColor: const Color(0xFF1E1E1E),
          appBarTheme: const AppBarTheme(
            backgroundColor: Color(0xFF323233),
          ),
        ),
        home: const AppRouter(),
      ),
    );
  }
}

class AppRouter extends StatelessWidget {
  const AppRouter({super.key});

  @override
  Widget build(BuildContext context) {
    final connection = context.watch<ConnectionProvider>();

    if (connection.isConnected) {
      return const WorkspaceScreen();
    }
    if (connection.activeServer != null) {
      return const PairingScreen();
    }
    return const ServerListScreen();
  }
}
