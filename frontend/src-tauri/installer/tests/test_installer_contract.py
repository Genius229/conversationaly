import json
import unittest
from pathlib import Path


SRC_TAURI = Path(__file__).resolve().parents[2]


def read(relative: str) -> str:
    return (SRC_TAURI / relative).read_text(encoding="utf-8")


class InstallerContractTests(unittest.TestCase):
    def test_gigastt_overlays_pin_reviewed_template_and_hooks(self) -> None:
        for overlay in (
            "tauri.gigastt.windows.conf.json",
            "tauri.gigastt.ci.conf.json",
        ):
            config = json.loads(read(overlay))
            nsis = config["bundle"]["windows"]["nsis"]
            self.assertEqual(
                nsis["template"], "installer/gigastt-installer.nsi", overlay
            )
            self.assertEqual(
                nsis["installerHooks"],
                "installer/gigastt-installer-hooks.nsh",
                overlay,
            )

    def test_no_basename_kills_or_old_nsis_uninstaller(self) -> None:
        template = read("installer/gigastt-installer.nsi")

        self.assertIn(
            "TAURI CLI 2.11.4 UPSTREAM SHA256: "
            "20f4ecc730defb71f1342eaeaec4021df13be3d843abba0effe88ea5835fa079",
            template,
        )
        self.assertIn(
            '!insertmacro CONVERSATIONALY_PREFLIGHT "reinstall"', template
        )
        self.assertIn("!insertmacro CONVERSATIONALY_DEPLOY_STAGED_PAYLOAD", template)
        self.assertNotIn("!insertmacro CheckIfAppIsRunning", template)
        self.assertNotIn("ExecWait '$R1'", template)

        preflight = template.index(
            '!insertmacro CONVERSATIONALY_PREFLIGHT "reinstall"'
        )
        self.assertLess(preflight, template.index("Section Install"))

    def test_payload_is_staged_before_transactional_deploy(self) -> None:
        template = read("installer/gigastt-installer.nsi")
        install = template.split("Section Install", 1)[1].split("SectionEnd", 1)[0]

        self.assertIn(
            'SetOutPath "$PLUGINSDIR\\conversationaly-payload"', install
        )
        self.assertIn(
            'CreateDirectory "$PLUGINSDIR\\conversationaly-payload\\{{this}}"',
            install,
        )
        self.assertNotIn('CreateDirectory "$INSTDIR\\\\{{this}}"', install)
        self.assertLess(
            install.rindex("  File "),
            install.index("!insertmacro CONVERSATIONALY_DEPLOY_STAGED_PAYLOAD"),
        )
        self.assertIn(
            'WriteUninstaller "$PLUGINSDIR\\conversationaly-payload\\uninstall.exe"',
            install,
        )
        self.assertNotIn('WriteUninstaller "$INSTDIR\\uninstall.exe"', install)

    def test_hooks_have_one_retry_cancel_surface_and_silent_exit_codes(self) -> None:
        hooks = read("installer/gigastt-installer-hooks.nsh")

        self.assertIn("MB_RETRYCANCEL|MB_ICONEXCLAMATION", hooks)
        self.assertNotIn("MB_ABORTRETRYIGNORE", hooks)
        self.assertNotIn("taskkill", hooks.lower())
        self.assertNotIn("/IM", hooks)
        self.assertIn("SetErrorLevel $", hooks)
        self.assertIn("${Silent}", hooks)
        self.assertIn('"Preflight"', hooks)
        self.assertIn('"Deploy"', hooks)

    def test_helper_declares_bounded_path_scoped_failure_contract(self) -> None:
        helper = read("installer/gigastt-installer-preflight.ps1")

        for exit_code in (10, 11, 12, 13):
            self.assertIn(f"exit {exit_code}", helper)
        self.assertIn("--installer-quit", helper)
        self.assertIn("--installer-target", helper)
        self.assertIn("[StringComparison]::OrdinalIgnoreCase", helper)
        self.assertIn("[IO.FileShare]::None", helper)
        self.assertIn("[IO.FileAttributes]::ReadOnly", helper)
        self.assertIn("[IO.File]::Move", helper)
        self.assertIn("ParentProcessId", helper)
        self.assertNotIn("taskkill", helper.lower())
        self.assertNotIn("Stop-Process -Name", helper)
        self.assertNotIn("return ,@($capturedPids)", helper)
        self.assertIn("[int[]]$mainPids = @(Request-MainShutdown $mainPath)", helper)


if __name__ == "__main__":
    unittest.main()
