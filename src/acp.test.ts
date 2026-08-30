import { assertEquals } from 'jsr:@std/assert@1';

// Pure logic tests for ACP GUI components (decision 25).
// No DOM rendering — just data transformations and validation.

/** Status → badge label mapping (mirrors AgentBadge.tsx STATUS_BADGE). */
const STATUS_BADGE: Record<string, { label: string }> = {
  pending: { label: 'queued' },
  running: { label: 'running' },
  completed: { label: 'done' },
  failed: { label: 'failed' },
  cancelled: { label: 'cancelled' },
};

/** Filter skill names to only those in the agent's skill list (mirrors
 * SkillPicker.tsx agentSkillManifests logic). */
function filterAgentSkills(
  available: { name: string; description: string }[],
  agentSkills: string[],
): { name: string; description: string }[] {
  const map = new Map(available.map((s) => [s.name, s]));
  return agentSkills
    .map((name) => map.get(name))
    .filter((s): s is { name: string; description: string } => s !== undefined);
}

/** Validate agent form: empty name is invalid, empty command is invalid
 * for non-built-in agents (mirrors AgentRegistry.tsx save logic). */
function validateAgentForm(name: string, command: string, isBuiltIn: boolean): string | null {
  if (!name.trim()) return 'Name cannot be empty';
  if (!command.trim() && !isBuiltIn) return 'Command cannot be empty for non-built-in agents';
  return null;
}

/** Parse skills_json from a run (mirrors AgentRunPanel.tsx skillsUsed). */
function parseSkillsUsed(skillsJson: string): string[] {
  try {
    const parsed = JSON.parse(skillsJson);
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

/** Check if a run status is terminal (mirrors AgentRunPanel.tsx isTerminal). */
function isTerminal(status: string): boolean {
  return status === 'completed' || status === 'failed' || status === 'cancelled';
}

/** Check if a run status is active (pending or running). */
function isActive(status: string): boolean {
  return status === 'pending' || status === 'running';
}

Deno.test('AgentBadge: status → label mapping', () => {
  assertEquals(STATUS_BADGE['pending'].label, 'queued');
  assertEquals(STATUS_BADGE['running'].label, 'running');
  assertEquals(STATUS_BADGE['completed'].label, 'done');
  assertEquals(STATUS_BADGE['failed'].label, 'failed');
  assertEquals(STATUS_BADGE['cancelled'].label, 'cancelled');
});

Deno.test('AgentBadge: unknown status falls back to pending', () => {
  const badge = STATUS_BADGE['unknown'] ?? STATUS_BADGE['pending'];
  assertEquals(badge.label, 'queued');
});

Deno.test('SkillPicker: filter available skills to agent skills only', () => {
  const available = [
    { name: 'ponytail', description: 'lazy mode' },
    { name: 'tdd', description: 'test-driven' },
    { name: 'research', description: 'research skill' },
  ];
  const agentSkills = ['ponytail', 'tdd', 'nonexistent'];
  const filtered = filterAgentSkills(available, agentSkills);
  assertEquals(filtered.length, 2);
  assertEquals(filtered[0].name, 'ponytail');
  assertEquals(filtered[1].name, 'tdd');
});

Deno.test('SkillPicker: empty agent skills → empty result', () => {
  const available = [{ name: 'ponytail', description: 'lazy mode' }];
  const filtered = filterAgentSkills(available, []);
  assertEquals(filtered.length, 0);
});

Deno.test('SkillPicker: no available skills → empty result', () => {
  const filtered = filterAgentSkills([], ['ponytail']);
  assertEquals(filtered.length, 0);
});

Deno.test('AgentRegistry: validate form — empty name rejected', () => {
  assertEquals(validateAgentForm('', 'claude-code', false), 'Name cannot be empty');
  assertEquals(validateAgentForm('  ', 'claude-code', false), 'Name cannot be empty');
});

Deno.test('AgentRegistry: validate form — empty command rejected for non-built-in', () => {
  assertEquals(
    validateAgentForm('my-agent', '', false),
    'Command cannot be empty for non-built-in agents',
  );
});

Deno.test('AgentRegistry: validate form — empty command allowed for built-in', () => {
  assertEquals(validateAgentForm('claude-code', '', true), null);
});

Deno.test('AgentRegistry: validate form — valid input passes', () => {
  assertEquals(validateAgentForm('my-agent', '/usr/bin/agent', false), null);
});

Deno.test('AgentRunPanel: parse skills_json valid array', () => {
  assertEquals(parseSkillsUsed('["ponytail","tdd"]'), ['ponytail', 'tdd']);
});

Deno.test('AgentRunPanel: parse skills_json empty array', () => {
  assertEquals(parseSkillsUsed('[]'), []);
});

Deno.test('AgentRunPanel: parse skills_json invalid JSON → empty', () => {
  assertEquals(parseSkillsUsed('not json'), []);
});

Deno.test('AgentRunPanel: parse skills_json null → empty', () => {
  assertEquals(parseSkillsUsed('null'), []);
});

Deno.test('AgentRunPanel: isTerminal — completed/failed/cancelled are terminal', () => {
  assertEquals(isTerminal('completed'), true);
  assertEquals(isTerminal('failed'), true);
  assertEquals(isTerminal('cancelled'), true);
});

Deno.test('AgentRunPanel: isTerminal — pending/running are not terminal', () => {
  assertEquals(isTerminal('pending'), false);
  assertEquals(isTerminal('running'), false);
});

Deno.test('AgentRunPanel: isActive — pending/running are active', () => {
  assertEquals(isActive('pending'), true);
  assertEquals(isActive('running'), true);
});

Deno.test('AgentRunPanel: isActive — terminal states are not active', () => {
  assertEquals(isActive('completed'), false);
  assertEquals(isActive('failed'), false);
  assertEquals(isActive('cancelled'), false);
});

Deno.test('Board polling: should continue while active runs exist', () => {
  const runs = [
    { status: 'running' },
    { status: 'completed' },
  ];
  const hasActive = runs.some((r) => isActive(r.status));
  assertEquals(hasActive, true);
});

Deno.test('Board polling: should stop when all runs are terminal', () => {
  const runs = [
    { status: 'completed' },
    { status: 'failed' },
  ];
  const hasActive = runs.some((r) => isActive(r.status));
  assertEquals(hasActive, false);
});

Deno.test('Board polling: empty runs → no active', () => {
  const runs: { status: string }[] = [];
  const hasActive = runs.some((r) => isActive(r.status));
  assertEquals(hasActive, false);
});

/** Parse unified diff text into hunk strings (mirrors AgentRunPanel.parseHunks). */
function parseHunks(diffText: string): string[] {
  const lines = diffText.split('\n');
  const hunks: string[] = [];
  let current: string[] = [];
  for (const line of lines) {
    if (line.startsWith('@@')) {
      if (current.length > 0) hunks.push(current.join('\n'));
      current = [line];
    } else if (current.length > 0) {
      current.push(line);
    }
  }
  if (current.length > 0) hunks.push(current.join('\n'));
  return hunks;
}

Deno.test('parseHunks: empty diff → no hunks', () => {
  assertEquals(parseHunks(''), []);
});

Deno.test('parseHunks: single hunk', () => {
  const diff = '@@ -1,3 +1,3 @@\n-old\n+new\n context';
  const hunks = parseHunks(diff);
  assertEquals(hunks.length, 1);
  assertEquals(hunks[0].startsWith('@@ -1,3 +1,3 @@'), true);
});

Deno.test('parseHunks: multiple hunks', () => {
  const diff = '@@ -1,3 +1,3 @@\n-old\n+new\n@@ -10,3 +10,3 @@\n-old2\n+new2';
  const hunks = parseHunks(diff);
  assertEquals(hunks.length, 2);
  assertEquals(hunks[0].includes('-old'), true);
  assertEquals(hunks[1].includes('-old2'), true);
});

Deno.test('parseHunks: lines before first @@ are skipped', () => {
  const diff = 'diff --git a/foo b/foo\nindex abc..def 100644\n@@ -1,3 +1,3 @@\n-old\n+new';
  const hunks = parseHunks(diff);
  assertEquals(hunks.length, 1);
  assertEquals(hunks[0].startsWith('@@'), true);
});
