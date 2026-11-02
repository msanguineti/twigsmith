"""Exercise service guards in actual PowerShell with disposable service/engine fakes."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

PWSH = os.environ.get('TWIGSMITH_TEST_PWSH') or shutil.which('pwsh')
SCRIPT = Path(__file__).resolve().parents[1] / 'scripts/ci/start-existing-docker.ps1'
WRAPPER = r'''
$ErrorActionPreference='Stop'
$global:Starts=0
$global:FakeService=[pscustomobject]@{Name='docker';Status=$env:CASE_STATUS;StartType=$env:CASE_STARTUP}
$global:FakeService | Add-Member ScriptMethod Refresh {}
$global:FakeService | Add-Member ScriptMethod WaitForStatus { param($state,$timeout) $this.Status='Running' }
function global:Get-Service {
  [CmdletBinding()]param([string]$Name)
  if ($env:CASE_STATUS -eq 'missing') { throw 'Service does not exist' }
  return $global:FakeService
}
function global:Start-Service {
  [CmdletBinding()]param([string]$Name)
  $global:Starts++
  if ($env:CASE_START_FAIL -eq '1') { throw 'Start denied' }
  $global:FakeService.Status='Running'
}
& $env:TWIGSMITH_SERVICE_SCRIPT -EvidenceDirectory $env:TWIGSMITH_SERVICE_EVIDENCE
$status=$LASTEXITCODE
[IO.File]::WriteAllText((Join-Path $env:TWIGSMITH_SERVICE_EVIDENCE 'calls.json'),
  (@{starts=$global:Starts;startup=$global:FakeService.StartType} | ConvertTo-Json),
  [Text.UTF8Encoding]::new($false))
exit $status
'''


@unittest.skipUnless(PWSH and os.name != 'nt', 'requires actual PowerShell and a disposable POSIX engine executable')
class ExistingDockerServiceTests(unittest.TestCase):
    def test_only_stopped_enabled_service_is_started_without_reconfiguration(self):
        cases = [('Stopped', 'Manual', False, True, 1),
                 ('Running', 'Automatic', False, True, 0),
                 ('Stopped', 'Disabled', False, False, 0),
                 ('missing', 'Manual', False, False, 0),
                 ('Stopped', 'Manual', True, False, 1)]
        for state, startup, start_fail, expected, starts in cases:
            with self.subTest(state=state, startup=startup, start_fail=start_fail), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                engine = root / 'docker'
                engine.write_text("#!/bin/sh\nprintf 'windows\\n'\n")
                engine.chmod(0o755)
                evidence = root / 'evidence'
                env = dict(os.environ, CASE_STATUS=state, CASE_STARTUP=startup,
                           CASE_START_FAIL=str(int(start_fail)),
                           TWIGSMITH_SERVICE_SCRIPT=str(SCRIPT), TWIGSMITH_SERVICE_EVIDENCE=str(evidence),
                           PATH=str(root) + os.pathsep + os.environ.get('PATH', ''))
                result = subprocess.run([str(Path(PWSH).resolve()), '-NoLogo', '-NoProfile', '-Command', WRAPPER],
                                        env=env, capture_output=True, text=True, timeout=30)
                report = json.loads((evidence / 'docker-service.json').read_text())
                self.assertEqual(result.returncode, 0 if expected else 1, (result.stderr, report))
                calls = json.loads((evidence / 'calls.json').read_text())
                self.assertEqual(report['passed'], expected)
                self.assertEqual(calls, {'starts': starts, 'startup': startup})
                if expected:
                    self.assertEqual(report['after']['status'], 'Running')
                    self.assertEqual(report['engine'], {'exit': 0, 'output': 'windows'})
                else:
                    self.assertIn('error', report)
                    self.assertNotIn('engine', report)


if __name__ == '__main__':
    unittest.main()
