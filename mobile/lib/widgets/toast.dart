import 'package:flutter/material.dart';

import '../app/globals.dart';
import '../theme/app_theme.dart';
import '../theme/tokens.dart';
import 'app_icon.dart';

/// Short floating message at the bottom of the screen (like Mantine notifications on desktop).
void showToast(String message, {bool error = false, String? title}) {
  final m = messengerKey.currentState;
  final ctx = messengerKey.currentContext;
  if (m == null || ctx == null) return;
  final t = ChatTokens.of(ctx);
  m.hideCurrentSnackBar();
  m.showSnackBar(SnackBar(
    duration: Duration(milliseconds: error ? 4500 : 2500),
    backgroundColor: t.ink,
    margin: const EdgeInsets.fromLTRB(16, 0, 16, 16),
    content: Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        AppIcon(error ? 'alert' : 'okcircle', size: 18, color: error ? t.danger : t.accent),
        const SizedBox(width: 10),
        Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              if (title != null) Text(title, style: ts(14, weight: 700, color: t.pane)),
              Text(message, style: ts(13.5, weight: 500, color: t.pane, height: 1.35)),
            ],
          ),
        ),
      ],
    ),
  ));
}
