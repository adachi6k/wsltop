"""Fixed, one-shot SIGTERM helper. Executed by python3 -I; never installed."""
import errno
import os
import signal
import sys
import time


def terminate(pid, start, boot, pid_namespace, mount_namespace, uid, deadline):
    if not hasattr(os, "pidfd_open") or not hasattr(signal, "pidfd_send_signal"):
        return "unsupported"
    if pid <= 1 or start <= 0:
        return "unsupported"
    fd = None
    try:
        with open("/proc/sys/kernel/random/boot_id") as f:
            current_boot = f.read().strip()
        if (current_boot != boot
                or os.readlink("/proc/self/ns/pid") != pid_namespace
                or os.readlink("/proc/self/ns/mnt") != mount_namespace
                or os.geteuid() != uid):
            return "stale"
        # Open before checking /proc. All signaling uses this handle; exit and
        # PID reuse after validation cannot redirect the signal to a successor.
        fd = os.pidfd_open(pid)
        with open("/proc/%d/stat" % pid) as f:
            fields = f.read().rsplit(")", 1)[1].split()
        if int(fields[19]) != start:
            return "stale"
        # Check the target's namespaces too: do not act on a container/nested
        # namespace process that happened to appear in primary /proc.
        if (os.readlink("/proc/%d/ns/pid" % pid) != pid_namespace
                or os.readlink("/proc/%d/ns/mnt" % pid) != mount_namespace):
            return "stale"
        if time.time() >= deadline:
            return "expired"
        signal.pidfd_send_signal(fd, signal.SIGTERM)
        return "accepted"
    except (ProcessLookupError, FileNotFoundError):
        return "stale"
    except PermissionError:
        return "denied"
    except OSError as error:
        if error.errno in (errno.ENOSYS, errno.EINVAL):
            return "unsupported"
        return "failed"
    except (ValueError, IndexError):
        return "stale"
    finally:
        if fd is not None:
            os.close(fd)


if __name__ == "__main__":
    try:
        result = terminate(int(sys.argv[1]), int(sys.argv[2]), sys.argv[3],
                           sys.argv[4], sys.argv[5], int(sys.argv[6]),
                           float(sys.argv[7]))
    except (ValueError, IndexError):
        result = "failed"
    print("WSLTOP_ACTION:" + result)
    sys.exit(0 if result == "accepted" else 1)
