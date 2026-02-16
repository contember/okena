import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'package:provider/provider.dart';

import 'src/providers/connection_provider.dart';
import 'src/providers/workspace_provider.dart';
import 'src/screens/server_list_screen.dart';
import 'src/screens/pairing_screen.dart';
import 'src/screens/workspace_screen.dart';
import 'src/theme/app_theme.dart';
import 'src/rust/frb_generated.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();

  await RustLib.init(
    externalLibrary: Platform.isIOS
        ? ExternalLibrary.process(iKnowHowToUseIt: true)
        : null,
  );

  // Edge-to-edge, dark status bar
  SystemChrome.setSystemUIOverlayStyle(const SystemUiOverlayStyle(
    statusBarColor: Colors.transparent,
    statusBarBrightness: Brightness.dark,
    statusBarIconBrightness: Brightness.light,
    systemNavigationBarColor: OkenaColors.background,
  ));
  SystemChrome.setEnabledSystemUIMode(SystemUiMode.edgeToEdge);

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
          scaffoldBackgroundColor: OkenaColors.background,
          colorScheme: const ColorScheme.dark(
            primary: OkenaColors.accent,
            surface: OkenaColors.surface,
            error: OkenaColors.error,
          ),
          appBarTheme: const AppBarTheme(
            backgroundColor: Colors.transparent,
            elevation: 0,
            scrolledUnderElevation: 0,
          ),
          bottomSheetTheme: const BottomSheetThemeData(
            backgroundColor: OkenaColors.surface,
            shape: RoundedRectangleBorder(
              borderRadius: BorderRadius.vertical(top: Radius.circular(16)),
            ),
            dragHandleColor: OkenaColors.textTertiary,
            showDragHandle: true,
          ),
          inputDecorationTheme: InputDecorationTheme(
            filled: true,
            fillColor: OkenaColors.surfaceElevated,
            border: OutlineInputBorder(
              borderRadius: BorderRadius.circular(12),
              borderSide: BorderSide.none,
            ),
            enabledBorder: OutlineInputBorder(
              borderRadius: BorderRadius.circular(12),
              borderSide: const BorderSide(color: OkenaColors.border, width: 0.5),
            ),
            focusedBorder: OutlineInputBorder(
              borderRadius: BorderRadius.circular(12),
              borderSide: const BorderSide(color: OkenaColors.accent, width: 1),
            ),
            labelStyle: OkenaTypography.callout,
            hintStyle: OkenaTypography.callout.copyWith(color: OkenaColors.textTertiary),
            contentPadding: const EdgeInsets.symmetric(horizontal: 16, vertical: 14),
          ),
          filledButtonTheme: FilledButtonThemeData(
            style: FilledButton.styleFrom(
              backgroundColor: OkenaColors.accent,
              foregroundColor: Colors.white,
              minimumSize: const Size(double.infinity, 50),
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(12),
              ),
              textStyle: OkenaTypography.headline,
            ),
          ),
          outlinedButtonTheme: OutlinedButtonThemeData(
            style: OutlinedButton.styleFrom(
              foregroundColor: OkenaColors.textPrimary,
              side: const BorderSide(color: OkenaColors.border),
              minimumSize: const Size(double.infinity, 50),
              shape: RoundedRectangleBorder(
                borderRadius: BorderRadius.circular(12),
              ),
              textStyle: OkenaTypography.headline,
            ),
          ),
          pageTransitionsTheme: const PageTransitionsTheme(
            builders: {
              TargetPlatform.iOS: CupertinoPageTransitionsBuilder(),
              TargetPlatform.android: CupertinoPageTransitionsBuilder(),
            },
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

    Widget child;
    if (connection.isConnected) {
      child = const WorkspaceScreen(key: ValueKey('workspace'));
    } else if (connection.activeServer != null) {
      child = const PairingScreen(key: ValueKey('pairing'));
    } else {
      child = const ServerListScreen(key: ValueKey('servers'));
    }

    return AnimatedSwitcher(
      duration: const Duration(milliseconds: 300),
      transitionBuilder: (child, animation) {
        return FadeTransition(
          opacity: animation,
          child: SlideTransition(
            position: Tween<Offset>(
              begin: const Offset(0, 0.02),
              end: Offset.zero,
            ).animate(CurvedAnimation(
              parent: animation,
              curve: Curves.easeOutCubic,
            )),
            child: child,
          ),
        );
      },
      child: child,
    );
  }
}
