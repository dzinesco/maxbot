// v2.4.0 — canonical `@BotName` mention parser used by
// both the Composer (to decide which Bots to run) and the
// React side (to render mention badges on transcript
// rows). The Rust side has an equivalent parser for the
// handoff tag; this file is the JS counterpart.
//
// Matching rules (per plan):
//   - Case-insensitive
//   - Whole-word (`\b` boundaries)
//   - Exact name match (no prefix matching for v2.4)
//   - De-duplicated, order preserved (first occurrence wins)

export interface MentionMember {
  /** Bot id, used to look up the actual run target. */
  id: string;
  /** Bot display name, matched against `@<name>`. */
  name: string;
}

/**
 * Extract the ids of Bots `@`-mentioned in `text`.
 *
 * Whitespace, punctuation, and the start / end of the
 * string are treated as word boundaries. The match is
 * case-insensitive against `member.name`. Unknown names
 * are silently dropped — a stray `@random` from the user
 * doesn't error, it just doesn't route.
 *
 * The returned list preserves the order of FIRST
 * OCCURRENCE in `text` (so `@Writer … @Researcher`
 * yields `[writer_id, researcher_id]`), and de-duplicates
 * so the Composer doesn't kick off the same Bot twice.
 *
 * Example:
 *   parseMentions("@Researcher find the Q3 numbers", members)
 *   // -> [members.find(m => m.name.toLowerCase() === "researcher")!.id]
 */
export function parseMentions(
  text: string,
  members: MentionMember[],
): string[] {
  if (!text || members.length === 0) return [];
  // Lower-case once for the case-insensitive match.
  const haystack = text.toLowerCase();
  // For each member, find the first `@<name>\b` match
  // position. We sort candidates by that position
  // (lowest first) so the output reflects "order of
  // first occurrence" regardless of the input member
  // list ordering. Bots with no match are dropped.
  type Hit = { id: string; pos: number };
  const hits: Hit[] = [];
  for (const m of members) {
    if (!m.name) continue;
    const nameLower = m.name.toLowerCase();
    const pattern = new RegExp(
      `@${escapeRegex(nameLower)}\\b`,
      "g",
    );
    const m1 = pattern.exec(haystack);
    if (m1) {
      hits.push({ id: m.id, pos: m1.index });
    }
  }
  hits.sort((a, b) => a.pos - b.pos);
  // De-duplicate by id while preserving the sorted
  // order (a member mentioned multiple times keeps its
  // first-occurrence slot).
  const seen = new Set<string>();
  const out: string[] = [];
  for (const h of hits) {
    if (!seen.has(h.id)) {
      seen.add(h.id);
      out.push(h.id);
    }
  }
  return out;
}

/**
 * Escape a string for use in a `RegExp` constructor.
 * Mirrors the common `[\^$.*+?()[\]{}|]` escape, kept
 * local so we don't pull in lodash.
 */
function escapeRegex(s: string): string {
  return s.replace(/[\\^$.*+?()[\]{}|]/g, "\\$&");
}
