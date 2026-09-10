import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


SRC_TAURI = Path(__file__).resolve().parents[2]


def read(relative: str) -> str:
    return (SRC_TAURI / relative).read_text(encoding="utf-8")


def render_one_handlebars_path_token(source: str, token: str, value: str) -> str:
    """Render one token with Handlebars' odd-backslash escape semantics."""
    index = source.index(token)
    slash_start = index
    while slash_start > 0 and source[slash_start - 1] == "\\":
        slash_start -= 1
    slash_count = index - slash_start
    prefix = source[:slash_start] + ("\\" * (slash_count // 2))
    if slash_count % 2:
        return prefix + token + source[index + len(token) :]
    return prefix + value + source[index + len(token) :]


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

    def test_maintenance_page_offers_only_truthful_in_place_install_or_cancel(self) -> None:
        template = read("installer/gigastt-installer.nsi")
        page = template.split("Function PageReinstall", 1)[1].split(
            "FunctionEnd", 1
        )[0]
        leave = template.split("Function PageLeaveReinstall", 1)[1].split(
            "FunctionEnd", 1
        )[0]

        self.assertIn("Reinstall in place", page)
        self.assertIn("Update in place", page)
        self.assertIn("Cancel setup", page)
        self.assertNotIn("$(uninstallApp)", page)
        self.assertIn("${BST_CHECKED}", leave)
        self.assertIn("SetErrorLevel 1", leave)

    def test_payload_is_staged_before_transactional_deploy(self) -> None:
        template = read("installer/gigastt-installer.nsi")
        install = template.split("Section Install", 1)[1].split("SectionEnd", 1)[0]

        self.assertIn(
            'SetOutPath "$PLUGINSDIR\\conversationaly-payload"', install
        )
        self.assertIn(
            '/oname=$PLUGINSDIR\\conversationaly-payload\\${MAINBINARYNAME}.exe',
            install,
        )
        resource_file_line = next(
            line
            for line in install.splitlines()
            if "File /a" in line and "{{this.[1]}}" in line
        )
        rendered_resource_file = render_one_handlebars_path_token(
            resource_file_line, "{{this.[1]}}", "gigastt\\DirectML.dll"
        )
        self.assertIn(
            '/oname=$PLUGINSDIR\\conversationaly-payload\\gigastt\\DirectML.dll',
            rendered_resource_file,
        )

        binary_file_line = next(
            line
            for line in install.splitlines()
            if "File /a" in line and "{{this}}" in line
        )
        rendered_binary_file = render_one_handlebars_path_token(
            binary_file_line, "{{this}}", "ffmpeg.exe"
        )
        self.assertIn(
            '/oname=$PLUGINSDIR\\conversationaly-payload\\ffmpeg.exe',
            rendered_binary_file,
        )

        resource_dir_line = next(
            line
            for line in install.splitlines()
            if "CreateDirectory" in line and "{{this}}" in line
        )
        rendered_resource_dir = render_one_handlebars_path_token(
            resource_dir_line, "{{this}}", "gigastt"
        )
        self.assertIn(
            '$PLUGINSDIR\\conversationaly-payload\\gigastt',
            rendered_resource_dir,
        )
        for directory in ("gigastt", "templates"):
            self.assertIn(
                f'CreateDirectory "$PLUGINSDIR\\conversationaly-payload\\{directory}"',
                install,
            )
        self.assertIn("ClearErrors", install)
        self.assertIn("${If} ${Errors}", install)
        self.assertIn("SetErrorLevel 13", install)
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
        self.assertIn(
            '!define CONVERSATIONALY_INSTALLER_HOOK_DIR "${__FILEDIR__}"', hooks
        )
        self.assertIn(
            '"${CONVERSATIONALY_INSTALLER_HOOK_DIR}\\gigastt-installer-preflight.ps1"',
            hooks,
        )
        self.assertNotIn(
            '"${__FILEDIR__}\\gigastt-installer-preflight.ps1"', hooks
        )
        self.assertNotIn("MB_ABORTRETRYIGNORE", hooks)
        self.assertNotIn("taskkill", hooks.lower())
        self.assertNotIn("/IM", hooks)
        self.assertIn("SetErrorLevel $", hooks)
        self.assertIn("${Silent}", hooks)
        self.assertIn('"Preflight"', hooks)
        self.assertIn('"Deploy"', hooks)
        for exit_code in ("0", "10", "11", "12", "13"):
            self.assertIn(f'StrCmp $8 "{exit_code}"', hooks)
        self.assertIn("StrCpy $8 13", hooks)

    def test_helper_declares_bounded_path_scoped_failure_contract(self) -> None:
        helper = read("installer/gigastt-installer-preflight.ps1")

        for exit_code in (10, 11, 12, 13):
            self.assertIn(f"exit {exit_code}", helper)
        self.assertIn("--installer-quit", helper)
        self.assertIn("--installer-target", helper)
        self.assertIn("[StringComparison]::OrdinalIgnoreCase", helper)
        self.assertIn("[IO.FileShare]::None", helper)
        self.assertIn("[IO.FileAttributes]::ReadOnly", helper)
        self.assertIn("Get-ChildItem -LiteralPath $InstallRoot -Directory -Recurse -Force", helper)
        self.assertIn("[IO.File]::Move", helper)
        self.assertIn("ParentProcessId", helper)
        self.assertNotIn("taskkill", helper.lower())
        self.assertNotIn("Stop-Process -Name", helper)
        self.assertNotIn("return ,@($capturedPids)", helper)
        self.assertIn("[int[]]$mainPids = @(Request-MainShutdown $mainPath)", helper)
        self.assertIn('$managedRuntimeRoot = Join-Path $InstallRoot "gigastt"', helper)
        self.assertIn('$oldInventoryPath = Join-Path $managedRuntimeRoot "runtime-inventory.json"', helper)
        self.assertIn("ConvertFrom-Json", helper)
        self.assertIn("[IO.Path]::GetFileName($oldName)", helper)
        self.assertIn("$payloadPathSet.ContainsKey($relative)", helper)
        self.assertNotIn(
            "Get-ChildItem -LiteralPath $managedRuntimeRoot -File -Recurse -Force",
            helper,
        )

    def test_deploy_removes_only_stale_old_inventory_paths(self) -> None:
        powershell = shutil.which("pwsh") or shutil.which("powershell.exe")
        bundled_pwsh = (
            SRC_TAURI.parents[1] / "tools/gigastt-contract-tests/target/pwsh/pwsh"
        )
        if powershell is None and bundled_pwsh.is_file():
            powershell = str(bundled_pwsh)
        if powershell is None:
            self.skipTest("PowerShell is unavailable")

        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            install = root / "install"
            runtime = install / "gigastt"
            payload = root / "payload"
            payload_runtime = payload / "gigastt"
            runtime.mkdir(parents=True)
            payload_runtime.mkdir(parents=True)

            (install / "conversationaly.exe").write_bytes(b"old-main")
            (runtime / "obsolete.dll").write_bytes(b"old-stale")
            (runtime / "unknown-user-sentinel.bin").write_bytes(b"preserve-me")
            (runtime / "runtime-inventory.json").write_text(
                json.dumps({"packagedFiles": [{"name": "obsolete.dll"}]}),
                encoding="utf-8",
            )
            (payload / "conversationaly.exe").write_bytes(b"new-main")
            (payload_runtime / "DirectML.dll").write_bytes(b"new-dll")
            (payload_runtime / "runtime-inventory.json").write_text(
                json.dumps({"packagedFiles": [{"name": "DirectML.dll"}]}),
                encoding="utf-8",
            )

            result = subprocess.run(
                [
                    powershell,
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                    str(SRC_TAURI / "installer/gigastt-installer-preflight.ps1"),
                    "-Mode",
                    "Deploy",
                    "-InstallDir",
                    str(install),
                    "-MainBinaryName",
                    "conversationaly.exe",
                    "-PayloadDir",
                    str(payload),
                ],
                capture_output=True,
                text=True,
                timeout=30,
                check=False,
            )

            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertFalse((runtime / "obsolete.dll").exists())
            self.assertEqual(
                (runtime / "unknown-user-sentinel.bin").read_bytes(), b"preserve-me"
            )
            self.assertEqual((runtime / "DirectML.dll").read_bytes(), b"new-dll")

    def test_deploy_rejects_incomplete_staged_runtime_before_target_changes(self) -> None:
        powershell = shutil.which("pwsh") or shutil.which("powershell.exe")
        bundled_pwsh = (
            SRC_TAURI.parents[1] / "tools/gigastt-contract-tests/target/pwsh/pwsh"
        )
        if powershell is None and bundled_pwsh.is_file():
            powershell = str(bundled_pwsh)
        if powershell is None:
            self.skipTest("PowerShell is unavailable")

        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            install = root / "install"
            payload = root / "payload"
            runtime = payload / "gigastt"
            install.mkdir()
            runtime.mkdir(parents=True)
            (install / "conversationaly.exe").write_bytes(b"old-main")
            (payload / "conversationaly.exe").write_bytes(b"new-main")
            (runtime / "runtime-inventory.json").write_text(
                json.dumps({"packagedFiles": [{"name": "DirectML.dll"}]}),
                encoding="utf-8",
            )

            result = subprocess.run(
                [
                    powershell,
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-File",
                    str(SRC_TAURI / "installer/gigastt-installer-preflight.ps1"),
                    "-Mode",
                    "Deploy",
                    "-InstallDir",
                    str(install),
                    "-MainBinaryName",
                    "conversationaly.exe",
                    "-PayloadDir",
                    str(payload),
                ],
                capture_output=True,
                text=True,
                timeout=30,
                check=False,
            )

            self.assertEqual(result.returncode, 13, result.stdout + result.stderr)
            self.assertEqual(
                (install / "conversationaly.exe").read_bytes(), b"old-main"
            )
            self.assertFalse((install / "gigastt").exists())


if __name__ == "__main__":
    unittest.main()
