// Compute next semver from conventional commits since last tag.
// Outputs: "next=<version>" and "bump=<major|minor|patch|none>" to stdout.
// Exit 0 with bump=none when no conventional commits found.

async function run(cmd: string[], cwd?: string): Promise<string> {
  const p = new Deno.Command(cmd[0], {
    args: cmd.slice(1),
    cwd,
    stdout: "piped",
    stderr: "piped",
  });
  const out = await p.output();
  return new TextDecoder().decode(out.stdout).trim();
}

async function git(args: string[]): Promise<string> {
  return run(["git", ...args]);
}

// Resolve the highest-version tag across all refs (not just those
// reachable from HEAD). `git describe --tags --abbrev=0` only finds
// tags reachable from HEAD, so a tag created on the `releases` branch
// is invisible when running on `main` — causing the script to fall
// back to "no prior tag" and re-emit v1.0.0, colliding with the
// existing tag.
async function lastTag(): Promise<string | null> {
  const out = await git(["tag", "--list", "--sort=-v:refname"]).catch(() => "");
  const first = out.split("\n").find((l) => /^v?\d+\.\d+\.\d+/.test(l.trim()));
  return first?.trim() || null;
}

function parseSemver(tag: string): [number, number, number] {
  const m = tag.replace(/^v/, "").match(/^(\d+)\.(\d+)\.(\d+)/);
  if (!m) return [0, 0, 0];
  return [Number(m[1]), Number(m[2]), Number(m[3])];
}

function bumpKind(
  subject: string,
  body: string,
): "major" | "minor" | "patch" | "none" {
  // Breaking change: footer or ! after type
  if (body.includes("BREAKING CHANGE:") || /^[a-z]+(\(.+\))?!:/.test(subject)) {
    return "major";
  }
  if (/^feat(\(.+\))?:/.test(subject)) return "minor";
  if (/^fix(\(.+\))?:/.test(subject)) return "patch";
  return "none";
}

const tag = await lastTag();
const range = tag ? `${tag}..HEAD` : "HEAD";
// First-parent log, subject + body, no merges.
const log = await git([
  "log",
  "--no-merges",
  "--format=%H%n%s%n%b%n---END---",
  range,
]);

let bump: "major" | "minor" | "patch" | "none" = "none";
const entries = log.split("---END---\n").filter((e) => e.trim());
for (const entry of entries) {
  const lines = entry.split("\n");
  const subject = lines[1] ?? "";
  const body = lines.slice(2).join("\n");
  const k = bumpKind(subject, body);
  if (k === "major") {
    bump = "major";
    break;
  }
  if (k === "minor" && bump === "none") bump = "minor";
  if (k === "patch" && bump === "none") bump = "patch";
}

let next: string;
if (!tag) {
  // No prior tag → initial release v1.0.0.
  next = "1.0.0";
  bump = "minor"; // non-none so release workflow proceeds
} else {
  const [maj, min, pat] = parseSemver(tag);
  next = `${maj}.${min}.${pat}`;
  if (bump === "major") next = `${maj + 1}.0.0`;
  else if (bump === "minor") next = `${maj}.${min + 1}.0`;
  else if (bump === "patch") next = `${maj}.${min}.${pat + 1}`;
}

console.log(`next=${next}`);
console.log(`bump=${bump}`);
