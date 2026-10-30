"""State reconciliation examples; no remote operation is performed."""
import unittest

from release_recovery import next_step


class RecoveryTests(unittest.TestCase):
    def setUp(self):
        self.assets = {'archive': 'a' * 64, 'manifest': 'b' * 64, 'checksum': 'c' * 64}
        self.npm = 'd' * 64
        self.state = {'exclusive': True, 'release': 'absent', 'assets': {},
                      'npm': 'absent', 'channel': 'other', 'consumption_verified': False, 'site_promoted': False}

    def step(self):
        return next_step(self.assets, self.npm, self.state)

    def test_success_interruption_and_timeout_after_remote_success(self):
        self.state['release'] = 'published'
        self.assertEqual(self.step(), 'upload archive')
        # A timed-out upload is observed before retry: identical bytes advance.
        self.state['assets']['archive'] = self.assets['archive']
        self.assertEqual(self.step(), 'upload checksum')
        self.state['assets'].update(self.assets)
        self.assertEqual(self.step(), 'publish npm')
        self.state['npm'] = self.npm
        self.assertEqual(self.step(), 'verify public consumption')
        self.state['consumption_verified'] = True
        self.assertEqual(self.step(), 'promote channel')
        self.state['channel'] = 'expected'
        self.assertEqual(self.step(), 'promote site')
        self.state['site_promoted'] = True
        self.assertEqual(self.step(), 'complete')

    def test_conflict_and_unknown_stop_before_advice(self):
        self.state.update(release='draft', assets={'archive': 'x' * 64})
        self.assertIn('conflicting', self.step())
        self.state.update(release='published', assets=dict(self.assets), npm='e' * 64)
        self.assertIn('conflicting', self.step())
        self.state['npm'] = 'unknown'
        self.assertIn('unknown', self.step())
        self.state.update(npm='absent', assets={'archive': self.assets['archive']})
        self.assertEqual(self.step(), 'upload checksum')

    def test_duplicate_invocation_requires_exclusive_control(self):
        self.state['exclusive'] = False
        self.assertIn('exclusivity', self.step())

    def test_failed_promotion_is_retried_only_after_all_required_state(self):
        self.state.update(release='published', assets=dict(self.assets), npm=self.npm,
                          consumption_verified=True, channel='expected')
        self.assertEqual(self.step(), 'promote site')
        self.state['consumption_verified'] = False
        self.assertEqual(self.step(), 'verify public consumption')


if __name__ == '__main__':
    unittest.main()
