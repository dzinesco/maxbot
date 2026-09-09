// v2.4.0 — tests for the canonical @BotName mention
// parser. The plan calls for: case-insensitive,
// whole-word, exact-name match, dedup, order-preserving.

import { describe, expect, it } from "vitest";
import { parseMentions, type MentionMember } from "./mentions";

const members: MentionMember[] = [
  { id: "researcher_id", name: "Researcher" },
  { id: "writer_id", name: "Writer" },
  { id: "reviewer_id", name: "Reviewer" },
];

describe("parseMentions", () => {
  it("returns the id of a single member mentioned by exact name", () => {
    expect(
      parseMentions("@Researcher find the Q3 numbers", members),
    ).toEqual(["researcher_id"]);
  });

  it("is case-insensitive against the member name", () => {
    expect(parseMentions("@RESEARCHER hi", members)).toEqual([
      "researcher_id",
    ]);
    expect(parseMentions("@researcher hi", members)).toEqual([
      "researcher_id",
    ]);
    expect(parseMentions("@rEsEaRcHeR hi", members)).toEqual([
      "researcher_id",
    ]);
  });

  it("ignores names that aren't in the member list", () => {
    expect(parseMentions("@RandomBot hi", members)).toEqual([]);
    expect(parseMentions("@NotAReal one @Researcher two", members)).toEqual([
      "researcher_id",
    ]);
  });

  it("deduplicates a member mentioned multiple times", () => {
    expect(
      parseMentions(
        "@Researcher one, @Researcher two, @researcher three",
        members,
      ),
    ).toEqual(["researcher_id"]);
  });

  it("preserves the order of first occurrence", () => {
    expect(
      parseMentions(
        "@Writer first, @Researcher second, @Writer again",
        members,
      ),
    ).toEqual(["writer_id", "researcher_id"]);
  });

  it("returns multiple distinct mentions", () => {
    expect(
      parseMentions("@Researcher and @Writer please", members),
    ).toEqual(["researcher_id", "writer_id"]);
  });

  it("respects word boundaries (no substring match)", () => {
    // A Bot named "Search" should not match in
    // "@Researcher" — the `\b` boundary fires after
    // "Search", not after "Search" inside "Researcher".
    const ms: MentionMember[] = [{ id: "search_id", name: "Search" }];
    expect(parseMentions("@Researcher hi", ms)).toEqual([]);
    expect(parseMentions("hi @Search!", ms)).toEqual(["search_id"]);
  });

  it("returns an empty list for empty text", () => {
    expect(parseMentions("", members)).toEqual([]);
  });

  it("returns an empty list for an empty member list", () => {
    expect(parseMentions("@Researcher hi", [])).toEqual([]);
  });
});
