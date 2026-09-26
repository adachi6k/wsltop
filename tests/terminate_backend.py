"""Regression tests for the actual embedded helper; only disposable children."""
import errno
import os
from pathlib import Path
import runpy
import signal
import subprocess
import time
import unittest
from unittest.mock import patch, mock_open

backend = runpy.run_path(str(Path(__file__).parent.parent / "src" / "terminate.py"))
terminate = backend["terminate"]


class BoundaryTests(unittest.TestCase):
    def setUp(self):
        self.patches = [
            patch("builtins.open", mock_open(read_data="boot")),
            patch("os.readlink", side_effect=lambda path: "pid:[7]" if path.endswith("pid") else "mnt:[8]"),
            patch("os.geteuid", return_value=1000),
            patch("os.pidfd_open", return_value=42, create=True),
            patch("os.close"),
            patch("signal.pidfd_send_signal", create=True),
            patch("os.kill", side_effect=AssertionError("PID-based signaling is forbidden")),
        ]
        self.open, _, _, self.pidfd, self.close, self.send, _ = [p.start() for p in self.patches]
        self.addCleanup(lambda: [p.stop() for p in reversed(self.patches)])
        fields = ["0"] * 20
        fields[19] = "123"
        self.stat = "99 (tricky ) name) " + " ".join(fields)
        self.open.side_effect = lambda path: mock_open(read_data="boot" if path.endswith("boot_id") else self.stat)()

    def call(self, **overrides):
        args = dict(pid=99, start=123, boot="boot", pid_namespace="pid:[7]",
                    mount_namespace="mnt:[8]", uid=1000, deadline=time.time() + 30)
        args.update(overrides)
        return terminate(**args)

    def test_opens_handle_before_reading_stat_and_signals_only_handle(self):
        original = self.open.side_effect
        def read(path):
            if path.endswith("/stat"):
                self.pidfd.assert_called_once_with(99)
            return original(path)
        self.open.side_effect = read
        self.assertEqual(self.call(), "accepted")
        self.send.assert_called_once_with(42, signal.SIGTERM)
        self.close.assert_called_once_with(42)

    def test_changed_start_identity_never_signals(self):
        self.assertEqual(self.call(start=124), "stale")
        self.send.assert_not_called()
        self.close.assert_called_once_with(42)

    def test_scope_changes_never_open_a_target(self):
        for change in [dict(boot="other"), dict(pid_namespace="pid:[9]"),
                       dict(mount_namespace="mnt:[9]"), dict(uid=0)]:
            self.assertEqual(self.call(**change), "stale")
        self.pidfd.assert_not_called()

    def test_nested_namespace_is_rejected(self):
        with patch("os.readlink", side_effect=lambda p: "pid:[9]" if "/99/" in p else ("pid:[7]" if p.endswith("pid") else "mnt:[8]")):
            self.assertEqual(self.call(), "stale")
        self.send.assert_not_called()

    def test_target_exit_between_validation_and_signal(self):
        self.send.side_effect = ProcessLookupError()
        self.assertEqual(self.call(), "stale")
        self.send.assert_called_once_with(42, signal.SIGTERM)

    def test_permission_error_has_no_retry(self):
        self.send.side_effect = PermissionError()
        self.assertEqual(self.call(), "denied")
        self.assertEqual(self.send.call_count, 1)

    def test_unsupported_kernel_and_expired_deadline(self):
        self.pidfd.side_effect = OSError(errno.ENOSYS, "unavailable")
        self.assertEqual(self.call(), "unsupported")
        self.pidfd.side_effect = None
        self.assertEqual(self.call(deadline=0), "expired")
        self.send.assert_not_called()


@unittest.skipUnless(hasattr(os, "pidfd_open") and hasattr(signal, "pidfd_send_signal"), "pidfd unavailable")
class LiveTests(unittest.TestCase):
    def child(self, ignore=False):
        code = "import signal,time; "
        if ignore:
            code += "signal.signal(signal.SIGTERM, signal.SIG_IGN); "
        code += "print('ready',flush=True); time.sleep(30)"
        child = subprocess.Popen(["python3", "-I", "-c", code], stdout=subprocess.PIPE, text=True)
        self.assertEqual(child.stdout.readline().strip(), "ready")
        def cleanup():
            if child.poll() is None:
                child.kill()
            child.wait()
            child.stdout.close()
        self.addCleanup(cleanup)
        return child

    def request(self, child):
        stat = Path("/proc/%d/stat" % child.pid).read_text()
        return dict(pid=child.pid, start=int(stat.rsplit(")", 1)[1].split()[19]),
                    boot=Path("/proc/sys/kernel/random/boot_id").read_text().strip(),
                    pid_namespace=os.readlink("/proc/self/ns/pid"),
                    mount_namespace=os.readlink("/proc/self/ns/mnt"),
                    uid=os.geteuid(), deadline=time.time() + 30)

    def test_real_graceful_termination(self):
        child = self.child()
        self.assertEqual(terminate(**self.request(child)), "accepted")
        self.assertEqual(child.wait(timeout=3), -signal.SIGTERM)

    def test_accepted_does_not_mean_exited(self):
        child = self.child(ignore=True)
        self.assertEqual(terminate(**self.request(child)), "accepted")
        self.assertIsNone(child.poll())

    def test_live_exit_after_validation_keeps_other_child_alive(self):
        child = self.child()
        other = self.child()
        request = self.request(child)
        send = signal.pidfd_send_signal
        def exit_then_send(fd, sig):
            child.terminate()
            child.wait(timeout=3)
            return send(fd, sig)
        with patch("signal.pidfd_send_signal", side_effect=exit_then_send):
            self.assertEqual(terminate(**request), "stale")
        self.assertIsNone(other.poll())


if __name__ == "__main__":
    unittest.main()
