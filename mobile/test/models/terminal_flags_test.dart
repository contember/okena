import 'dart:ui';

import 'package:flutter/painting.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:mobile/src/widgets/terminal_painter.dart';

void main() {
  group('flagsToDecoration', () {
    test('no flags returns none', () {
      expect(flagsToDecoration(0), TextDecoration.none);
    });

    test('underline flag', () {
      expect(flagsToDecoration(4), TextDecoration.underline);
    });

    test('strikethrough flag', () {
      expect(flagsToDecoration(8), TextDecoration.lineThrough);
    });

    test('underline + strikethrough combined', () {
      final deco = flagsToDecoration(4 | 8);
      // TextDecoration.combine returns a combined decoration
      expect(deco.contains(TextDecoration.underline), isTrue);
      expect(deco.contains(TextDecoration.lineThrough), isTrue);
    });

    test('bold/italic flags do not add decoration', () {
      expect(flagsToDecoration(1), TextDecoration.none); // bold
      expect(flagsToDecoration(2), TextDecoration.none); // italic
      expect(flagsToDecoration(3), TextDecoration.none); // bold+italic
    });
  });

  group('argbToColor', () {
    test('opaque white', () {
      final c = argbToColor(0xFFFFFFFF);
      expect(c, const Color(0xFFFFFFFF));
    });

    test('opaque red', () {
      final c = argbToColor(0xFFFF0000);
      expect(c.r, closeTo(1.0, 0.01));
      expect(c.g, closeTo(0.0, 0.01));
      expect(c.b, closeTo(0.0, 0.01));
    });

    test('semi-transparent', () {
      final c = argbToColor(0x80000000);
      expect(c.a, closeTo(0.5, 0.01));
    });
  });
}
