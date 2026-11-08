"""Shared helpers for offline npm installer packaging."""
import hashlib
import os
from pathlib import Path


def sha256(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def package_filename(report, name, version):
    """npm 11 reports an array; npm 12 keys its report by package name."""
    if isinstance(report, dict) and set(report) == {name}:
        report = [report[name]]
    if (not isinstance(report, list) or len(report) != 1
            or not isinstance(report[0], dict)
            or report[0].get('filename') != f'{name}-{version}.tgz'):
        raise ValueError('unexpected npm pack output')
    return report[0]['filename']


def base_metadata(name, version):
    return {'name': name, 'version': version, 'license': 'MIT',
            'repository': {'type': 'git', 'url': 'https://github.com/msanguineti/twigsmith.git'},
            'homepage': 'https://msanguineti.github.io/twigsmith/'}


def npm_environment(root):
    root.mkdir(parents=True, exist_ok=True)
    home = root / 'home'
    home.mkdir()
    npmrc = home / 'npmrc'
    npmrc.write_text('')
    # Windows Node/npm require the system root and executable search path.
    env = {key: value for key, value in os.environ.items()
           if key.upper() in ('PATH', 'SYSTEMROOT', 'WINDIR', 'COMSPEC', 'PATHEXT', 'TEMP', 'TMP')}
    env.update({'HOME': str(home), 'USERPROFILE': str(home),
                'npm_config_userconfig': str(npmrc),
                'npm_config_globalconfig': str(home / 'global-npmrc'),
                'npm_config_cache': str(home / 'cache'),
                'npm_config_update_notifier': 'false', 'npm_config_ignore_scripts': 'true',
                'npm_config_offline': 'true'})
    return env


def npm_command(*args):
    # npm.cmd is a batch script, not a CreateProcess executable. Invoke npm's JS
    # entry point directly on Windows, avoiding cmd.exe argument re-parsing.
    if os.name == 'nt':
        import shutil
        executable = shutil.which('npm.cmd')
        if not executable:
            raise ValueError('npm.cmd was not found on PATH')
        cli = Path(executable).parent / 'node_modules/npm/bin/npm-cli.js'
        if not cli.is_file():
            raise ValueError('npm JavaScript entry point was not found beside npm.cmd')
        return ['node', str(cli), *args]
    return ['npm', *args]
