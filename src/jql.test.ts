/// <reference lib="deno.ns" />
import { DEFAULT_JQL_PARTS, type JqlParts, parseJqlParts } from "./jql.ts";

function assertEquals<T>(actual: T, expected: T, msg = ""): void {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a !== e) {
    throw new Error(
      `assertEquals failed${msg ? ` (${msg})` : ""}: expected ${e}, got ${a}`,
    );
  }
}

Deno.test("parseJqlParts: valid round-trips", () => {
  const parts: JqlParts = {
    project: "P",
    assigneeMode: "specific",
    assignee: "a",
    statusMode: "specific",
    statuses: ["s1"],
    updatedWithin: "30d",
    orderBy: "created",
  };
  assertEquals(parseJqlParts(JSON.stringify(parts)), parts);
});

Deno.test("parseJqlParts: unset → defaults", () => {
  assertEquals(parseJqlParts(undefined), DEFAULT_JQL_PARTS);
  assertEquals(parseJqlParts(""), DEFAULT_JQL_PARTS);
});

Deno.test("parseJqlParts: invalid JSON → defaults", () => {
  assertEquals(parseJqlParts("{not json"), DEFAULT_JQL_PARTS);
});

Deno.test("parseJqlParts: bad enum values fall back to defaults", () => {
  const raw = JSON.stringify({
    project: "P",
    assigneeMode: "bogus",
    statusMode: "bogus",
    updatedWithin: "bogus",
    orderBy: "bogus",
    statuses: "not-an-array",
  });
  const parsed = parseJqlParts(raw);
  assertEquals(parsed.assigneeMode, DEFAULT_JQL_PARTS.assigneeMode);
  assertEquals(parsed.statusMode, DEFAULT_JQL_PARTS.statusMode);
  assertEquals(parsed.updatedWithin, DEFAULT_JQL_PARTS.updatedWithin);
  assertEquals(parsed.orderBy, DEFAULT_JQL_PARTS.orderBy);
  assertEquals(parsed.statuses, []);
  assertEquals(parsed.project, "P");
});
