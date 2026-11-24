// The official-account badge: an account the server operator vouches for (signed by the
// operator, checked by the core; see Contact.official). It is drawn by the app, never taken from
// text, so a display name, an emoji or a profile text cannot imitate it, and it reads out as
// "Official account · <label>" to screen readers. It is a different thing from the safety number
// shield ('shield', the user compared keys): different icon, different words.

import 'package:flutter/material.dart';

import '../core/models.dart';
import '../l10n/i18n.dart';
import '../theme/app_theme.dart';
import 'app_icon.dart';
import 'ui.dart';

/// Whether [label] is a badge label worth showing (null or blank: no badge).
bool hasOfficialLabel(String? label) => label != null && label.trim().isNotEmpty;

/// Badge of a conversation's peer: the conversation's own field, or the contact's (whichever
/// the core updated last carries it). Groups never have one.
String? conversationOfficial(Conversation c, Map<String, Contact> contacts) {
  if (c.isGroup) return null;
  final own = c.official;
  if (hasOfficialLabel(own)) return own;
  final peer = c.peerAccountId == null ? null : contacts[c.peerAccountId];
  return hasOfficialLabel(peer?.official) ? peer!.official : null;
}

/// The badge icon next to a name. Renders nothing for a null or empty [label]. [gap] is the
/// space before it (on the start side, so it follows the name in right-to-left layouts too).
class OfficialBadge extends StatelessWidget {
  const OfficialBadge(this.label, {super.key, this.size = 16, this.gap = 6});

  final String? label;
  final double size;
  final double gap;

  @override
  Widget build(BuildContext context) {
    if (!hasOfficialLabel(label)) return const SizedBox.shrink();
    final text = context.t('mobile.official.label', args: {'label': label!.trim()});
    return Padding(
      padding: EdgeInsetsDirectional.only(start: gap),
      child: Semantics(
        label: text,
        image: true,
        container: true,
        child: ExcludeSemantics(
          child: Tooltip(message: text, excludeFromSemantics: true, child: OfficialSeal(size: size)),
        ),
      ),
    );
  }
}

/// The bare seal (brand color, check in the contrasting color), without semantics.
class OfficialSeal extends StatelessWidget {
  const OfficialSeal({super.key, this.size = 16});
  final double size;

  @override
  Widget build(BuildContext context) {
    final t = context.tk;
    return SizedBox.square(
      dimension: size,
      child: Stack(children: [
        AppIcon('seal', size: size, color: t.accent),
        AppIcon('sealcheck', size: size, color: t.onAccent, strokeWidth: 2.4),
      ]),
    );
  }
}

/// A line of text with the badge: "Official account · <label>", for the contact page, our own
/// profile and message requests. [text] replaces the default wording (the label must be in it).
class OfficialLine extends StatelessWidget {
  const OfficialLine(this.label, {super.key, this.text, this.hint, this.center = false});

  final String? label;
  final String? text;
  final String? hint;
  final bool center;

  @override
  Widget build(BuildContext context) {
    if (!hasOfficialLabel(label)) return const SizedBox.shrink();
    final t = context.tk;
    final line = text ?? context.t('mobile.official.label', args: {'label': label!.trim()});
    return Semantics(
      container: true,
      child: Column(
        crossAxisAlignment: center ? CrossAxisAlignment.center : CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Container(
            padding: const EdgeInsetsDirectional.fromSTEB(6, 4, 10, 4),
            decoration: BoxDecoration(color: t.accentSoft, borderRadius: BorderRadius.circular(999)),
            child: Row(mainAxisSize: MainAxisSize.min, children: [
              const ExcludeSemantics(child: OfficialSeal(size: 18)),
              const SizedBox(width: 6),
              Flexible(child: Text(line, style: ts(13, weight: 650, color: t.accentText))),
            ]),
          ),
          if (hint != null) ...[
            const SizedBox(height: 6),
            Text(hint!, textAlign: center ? TextAlign.center : TextAlign.start, style: ts(12.5, color: t.muted, height: 1.45)),
          ],
        ],
      ),
    );
  }
}
