using System;
using System.IO;
using System.Threading;

// Test-owned process, deliberately copied to names used by the real application.
// Runs OUTSIDE the install directory. A same-name process must never be killed
// by the installer. FileShare.Read permits snapshots but denies replacement.
internal static class LockFixture
{
    private static int Main(string[] args)
    {
        if (args.Length < 1 || args.Length > 2) return 2;
        using (var file = args.Length == 2
            ? new FileStream(args[1], FileMode.Open, FileAccess.Read, FileShare.Read)
            : null)
        {
            File.WriteAllText(args[0], "ready");
            Thread.Sleep(Timeout.Infinite);
        }
        return 0;
    }
}
