import 'package:flutter/material.dart';

import '../theme/app_theme.dart';
import 'app_icon.dart';
import 'ui.dart';

/// Segmented tabs of the QR modal ("My code" / "Scan"): sunk track, active tab on pane.
class SegTabs extends StatelessWidget {
  const SegTabs({super.key, required this.tabs, required this.value, required this.onChanged});

  final List<(String value, String icon, String label)> tabs;
  final String value;
  final ValueChanged<String> onChanged;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return Container(
      padding: const EdgeInsets.all(4),
      decoration: BoxDecoration(color: t.sunk, borderRadius: t.inputRadius(46)),
      child: Row(children: [
        for (final tab in tabs)
          Expanded(
            child: GestureDetector(
              behavior: HitTestBehavior.opaque,
              onTap: () => onChanged(tab.$1),
              child: AnimatedContainer(
                duration: const Duration(milliseconds: 150),
                height: 38,
                decoration: BoxDecoration(
                  color: value == tab.$1 ? t.pane : Colors.transparent,
                  borderRadius: t.inputRadius(38),
                  boxShadow: value == tab.$1 ? const [BoxShadow(color: Color.fromRGBO(0, 0, 0, .1), blurRadius: 3, offset: Offset(0, 1))] : null,
                ),
                child: Row(mainAxisAlignment: MainAxisAlignment.center, children: [
                  AppIcon(tab.$2, size: 16, color: value == tab.$1 ? t.ink : t.muted),
                  const SizedBox(width: 8),
                  Text(tab.$3, style: ts(14, weight: 650, color: value == tab.$1 ? t.ink : t.muted)),
                ]),
              ),
            ),
          ),
      ]),
    );
  }
}
