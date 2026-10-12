'use strict';

// GitHub-hosted runners do not contain HomeRun's patched Actions runner.
// Apply the same Results API URL override immediately before upload-artifact
// initializes its client, while keeping the hosted macOS build runner.
const value = process.env.ACTIONS_ARTIFACTS_RESULTS_URL_OVERRIDE;

if (typeof value !== 'string' || value.trim() === '') {
  throw new Error(
    'Set the repository Actions variable ACTIONS_ARTIFACTS_RESULTS_URL_OVERRIDE ' +
    'to the custom artifact server Results API URL.'
  );
}

const override = value.trim();
let parsed;
try {
  parsed = new URL(override);
} catch {
  throw new Error('ACTIONS_ARTIFACTS_RESULTS_URL_OVERRIDE must be a valid absolute URL.');
}

if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') {
  throw new Error('ACTIONS_ARTIFACTS_RESULTS_URL_OVERRIDE must use HTTP or HTTPS.');
}

// Mirrors the patched runner's NodeScriptActionHandler behavior.
process.env.ACTIONS_RESULTS_URL = override;
