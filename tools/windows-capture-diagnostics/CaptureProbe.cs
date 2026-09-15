using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Threading.Tasks;

public sealed class CaptureProbeResult
{
    public bool Started { get; set; }
    public string[] Arguments { get; set; }
    public DateTime StartedAtUtc { get; set; }
    public DateTime FinishedAtUtc { get; set; }
    public long DurationMilliseconds { get; set; }
    public long? FirstStdoutByteMilliseconds { get; set; }
    public int? ExitCode { get; set; }
    public bool ForcedStop { get; set; }
    public bool StopRequested { get; set; }
    public bool TimedOut { get; set; }
    public long TotalStdoutBytes { get; set; }
    public long TotalStderrBytes { get; set; }
    public byte[] RetainedStdout { get; set; }
    public byte[] RetainedStderr { get; set; }
    public bool StdoutTruncated { get; set; }
    public bool StderrTruncated { get; set; }
    public bool CleanupComplete { get; set; }
    public string CleanupError { get; set; }
    public string StopWriteError { get; set; }
    public bool JobAssigned { get; set; }
    public string Outcome { get; set; }
    public string LaunchError { get; set; }
}

public static class CaptureProbe
{
    private const int RetainedByteLimit = 128 * 1024;
    private const UInt32 JobObjectExtendedLimitInformationClass = 9;
    private const UInt32 JobObjectLimitKillOnJobClose = 0x00002000;

    private sealed class DrainResult
    {
        public long Total;
        public byte[] Retained;
        public bool Truncated;
        public Exception Error;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct IoCounters
    {
        public UInt64 ReadOperationCount;
        public UInt64 WriteOperationCount;
        public UInt64 OtherOperationCount;
        public UInt64 ReadTransferCount;
        public UInt64 WriteTransferCount;
        public UInt64 OtherTransferCount;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct JobObjectBasicLimitInformation
    {
        public Int64 PerProcessUserTimeLimit;
        public Int64 PerJobUserTimeLimit;
        public UInt32 LimitFlags;
        public UIntPtr MinimumWorkingSetSize;
        public UIntPtr MaximumWorkingSetSize;
        public UInt32 ActiveProcessLimit;
        public IntPtr Affinity;
        public UInt32 PriorityClass;
        public UInt32 SchedulingClass;
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct JobObjectExtendedLimitInformation
    {
        public JobObjectBasicLimitInformation BasicLimitInformation;
        public IoCounters IoInfo;
        public UIntPtr ProcessMemoryLimit;
        public UIntPtr JobMemoryLimit;
        public UIntPtr PeakProcessMemoryUsed;
        public UIntPtr PeakJobMemoryUsed;
    }

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr CreateJobObject(IntPtr jobAttributes, string name);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool SetInformationJobObject(
        IntPtr job,
        UInt32 informationClass,
        IntPtr information,
        UInt32 informationLength);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool CloseHandle(IntPtr handle);

    public static string QuoteArgument(string argument)
    {
        if (argument == null)
            throw new ArgumentNullException("argument");

        StringBuilder quoted = new StringBuilder();
        quoted.Append('"');
        int backslashes = 0;
        for (int i = 0; i < argument.Length; i++)
        {
            char value = argument[i];
            if (value == '\\')
            {
                backslashes++;
                continue;
            }

            if (value == '"')
            {
                quoted.Append('\\', backslashes * 2 + 1);
                quoted.Append('"');
                backslashes = 0;
                continue;
            }

            quoted.Append('\\', backslashes);
            backslashes = 0;
            quoted.Append(value);
        }

        quoted.Append('\\', backslashes * 2);
        quoted.Append('"');
        return quoted.ToString();
    }

    private static string BuildArguments(string[] arguments)
    {
        StringBuilder result = new StringBuilder();
        for (int i = 0; i < arguments.Length; i++)
        {
            if (i != 0)
                result.Append(' ');
            result.Append(QuoteArgument(arguments[i]));
        }
        return result.ToString();
    }

    private static bool IsWindows()
    {
        return Environment.OSVersion.Platform == PlatformID.Win32NT;
    }

    private static IntPtr CreateKillOnCloseJob()
    {
        if (!IsWindows())
            return IntPtr.Zero;

        IntPtr job = CreateJobObject(IntPtr.Zero, null);
        if (job == IntPtr.Zero)
            throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "CreateJobObject failed");

        IntPtr buffer = IntPtr.Zero;
        try
        {
            JobObjectExtendedLimitInformation info = new JobObjectExtendedLimitInformation();
            info.BasicLimitInformation.LimitFlags = JobObjectLimitKillOnJobClose;
            int length = Marshal.SizeOf(typeof(JobObjectExtendedLimitInformation));
            buffer = Marshal.AllocHGlobal(length);
            Marshal.StructureToPtr(info, buffer, false);
            if (!SetInformationJobObject(job, JobObjectExtendedLimitInformationClass, buffer, (UInt32)length))
                throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "SetInformationJobObject failed");
            return job;
        }
        catch
        {
            CloseHandle(job);
            throw;
        }
        finally
        {
            if (buffer != IntPtr.Zero)
                Marshal.FreeHGlobal(buffer);
        }
    }

    private static DrainResult Drain(Stream stream, bool retain, Stopwatch clock, CaptureProbeResult result, object firstByteLock)
    {
        DrainResult drained = new DrainResult();
        MemoryStream kept = new MemoryStream();
        byte[] buffer = new byte[8192];
        try
        {
            while (true)
            {
                int read = stream.Read(buffer, 0, buffer.Length);
                if (read == 0)
                    break;

                if (Object.ReferenceEquals(stream, null))
                    throw new InvalidOperationException("unreachable");

                if (result.FirstStdoutByteMilliseconds == null && stream.GetType() != typeof(Stream))
                {
                    // The caller sets first-byte timing for stdout through the
                    // explicit flag below. This branch is intentionally empty.
                }

                drained.Total += read;
                if (retain && kept.Length < RetainedByteLimit)
                {
                    int remaining = RetainedByteLimit - (int)kept.Length;
                    int count = Math.Min(remaining, read);
                    kept.Write(buffer, 0, count);
                }
            }
        }
        catch (Exception error)
        {
            drained.Error = error;
        }
        drained.Retained = kept.ToArray();
        drained.Truncated = drained.Total > drained.Retained.LongLength;
        kept.Dispose();
        return drained;
    }

    private static DrainResult DrainStdout(Stream stream, bool retain, Stopwatch clock, CaptureProbeResult result, object firstByteLock)
    {
        DrainResult drained = new DrainResult();
        MemoryStream kept = new MemoryStream();
        byte[] buffer = new byte[8192];
        try
        {
            while (true)
            {
                int read = stream.Read(buffer, 0, buffer.Length);
                if (read == 0)
                    break;
                lock (firstByteLock)
                {
                    if (result.FirstStdoutByteMilliseconds == null)
                        result.FirstStdoutByteMilliseconds = clock.ElapsedMilliseconds;
                }
                drained.Total += read;
                if (retain && kept.Length < RetainedByteLimit)
                {
                    int remaining = RetainedByteLimit - (int)kept.Length;
                    int count = Math.Min(remaining, read);
                    kept.Write(buffer, 0, count);
                }
            }
        }
        catch (Exception error)
        {
            drained.Error = error;
        }
        drained.Retained = kept.ToArray();
        drained.Truncated = drained.Total > drained.Retained.LongLength;
        kept.Dispose();
        return drained;
    }

    private static bool WaitUntilExit(Process process, int milliseconds)
    {
        if (milliseconds < 0)
            milliseconds = 0;
        try
        {
            return process.WaitForExit(milliseconds);
        }
        catch (InvalidOperationException)
        {
            return true;
        }
    }

    private static void RequestQuit(Process process, CaptureProbeResult result)
    {
        result.StopRequested = true;
        try
        {
            // StreamWriter on .NET Framework may emit an encoding preamble on
            // its first write. FFmpeg's interactive command is byte-oriented:
            // send exactly ASCII "q\n" through the underlying pipe.
            byte[] quit = new byte[] { 0x71, 0x0A };
            process.StandardInput.BaseStream.Write(quit, 0, quit.Length);
            process.StandardInput.BaseStream.Flush();
        }
        catch (Exception error)
        {
            result.StopWriteError = error.Message;
        }
    }

    private static void ForceStop(Process process, CaptureProbeResult result, int killGraceMilliseconds)
    {
        result.ForcedStop = true;
        try
        {
            if (!process.HasExited)
                process.Kill();
        }
        catch (Exception error)
        {
            result.CleanupError = "Kill failed: " + error.Message;
        }
        WaitUntilExit(process, killGraceMilliseconds);
    }

    private static CaptureProbeResult RunCore(
        string fileName,
        string[] arguments,
        int timeoutMilliseconds,
        bool retainStdout,
        bool captureMode,
        int firstDataTimeoutMilliseconds,
        int captureAfterFirstDataMilliseconds,
        int quitGraceMilliseconds,
        int killGraceMilliseconds)
    {
        if (String.IsNullOrWhiteSpace(fileName))
            throw new ArgumentException("An exact executable path is required", "fileName");
        if (arguments == null)
            throw new ArgumentNullException("arguments");

        CaptureProbeResult result = new CaptureProbeResult();
        result.Arguments = (string[])arguments.Clone();
        result.RetainedStdout = new byte[0];
        result.RetainedStderr = new byte[0];
        result.StartedAtUtc = DateTime.UtcNow;
        result.Outcome = "launch_failure";

        Stopwatch clock = Stopwatch.StartNew();
        Process process = null;
        IntPtr job = IntPtr.Zero;
        Task<DrainResult> stdoutTask = null;
        Task<DrainResult> stderrTask = null;
        object firstByteLock = new object();
        bool exitedBeforeStop = false;

        try
        {
            job = CreateKillOnCloseJob();
            ProcessStartInfo start = new ProcessStartInfo();
            start.FileName = fileName;
            start.Arguments = BuildArguments(arguments);
            start.UseShellExecute = false;
            start.CreateNoWindow = true;
            start.RedirectStandardInput = true;
            start.RedirectStandardOutput = true;
            start.RedirectStandardError = true;

            process = new Process();
            process.StartInfo = start;
            if (!process.Start())
                throw new InvalidOperationException("Process.Start returned false");
            result.Started = true;

            if (job != IntPtr.Zero)
            {
                if (!AssignProcessToJobObject(job, process.Handle))
                    throw new System.ComponentModel.Win32Exception(Marshal.GetLastWin32Error(), "AssignProcessToJobObject failed");
                result.JobAssigned = true;
            }

            stdoutTask = Task.Factory.StartNew(
                delegate { return DrainStdout(process.StandardOutput.BaseStream, retainStdout, clock, result, firstByteLock); },
                CancellationToken.None,
                TaskCreationOptions.LongRunning,
                TaskScheduler.Default);
            stderrTask = Task.Factory.StartNew(
                delegate { return Drain(process.StandardError.BaseStream, true, clock, result, firstByteLock); },
                CancellationToken.None,
                TaskCreationOptions.LongRunning,
                TaskScheduler.Default);

            if (!captureMode)
            {
                if (!WaitUntilExit(process, timeoutMilliseconds))
                {
                    result.TimedOut = true;
                    result.Outcome = "forced_timeout";
                    ForceStop(process, result, killGraceMilliseconds);
                }
                else
                {
                    exitedBeforeStop = true;
                    result.Outcome = "natural_exit";
                }
            }
            else
            {
                long firstDeadline = clock.ElapsedMilliseconds + firstDataTimeoutMilliseconds;
                bool firstData = false;
                while (clock.ElapsedMilliseconds < firstDeadline)
                {
                    lock (firstByteLock)
                        firstData = result.FirstStdoutByteMilliseconds != null;
                    if (firstData)
                        break;
                    if (WaitUntilExit(process, 20))
                    {
                        exitedBeforeStop = true;
                        break;
                    }
                }

                if (!exitedBeforeStop)
                {
                    lock (firstByteLock)
                        firstData = result.FirstStdoutByteMilliseconds != null;
                    if (firstData)
                    {
                        long stopAt = result.FirstStdoutByteMilliseconds.Value + captureAfterFirstDataMilliseconds;
                        while (clock.ElapsedMilliseconds < stopAt)
                        {
                            int remaining = (int)Math.Min(20, stopAt - clock.ElapsedMilliseconds);
                            if (WaitUntilExit(process, Math.Max(1, remaining)))
                            {
                                exitedBeforeStop = true;
                                break;
                            }
                        }
                        if (!exitedBeforeStop)
                        {
                            result.Outcome = "planned_stop_after_data";
                            RequestQuit(process, result);
                        }
                    }
                    else
                    {
                        result.TimedOut = true;
                        result.Outcome = "no_data_timeout";
                        RequestQuit(process, result);
                    }

                    if (!exitedBeforeStop && !WaitUntilExit(process, quitGraceMilliseconds))
                        ForceStop(process, result, killGraceMilliseconds);
                }
            }

            if (!process.HasExited)
                ForceStop(process, result, killGraceMilliseconds);

            if (process.HasExited)
                result.ExitCode = process.ExitCode;
        }
        catch (Exception error)
        {
            result.LaunchError = error.ToString();
            if (process != null && result.Started)
                ForceStop(process, result, killGraceMilliseconds);
        }
        finally
        {
            bool drainsComplete = true;
            try
            {
                if (stdoutTask != null && !stdoutTask.Wait(killGraceMilliseconds))
                    drainsComplete = false;
                if (stderrTask != null && !stderrTask.Wait(killGraceMilliseconds))
                    drainsComplete = false;

                if (stdoutTask != null && stdoutTask.IsCompleted)
                {
                    DrainResult stdout = stdoutTask.Result;
                    result.TotalStdoutBytes = stdout.Total;
                    result.RetainedStdout = stdout.Retained;
                    result.StdoutTruncated = stdout.Truncated;
                    if (stdout.Error != null && result.CleanupError == null)
                        result.CleanupError = "stdout drain failed: " + stdout.Error.Message;
                }
                if (stderrTask != null && stderrTask.IsCompleted)
                {
                    DrainResult stderr = stderrTask.Result;
                    result.TotalStderrBytes = stderr.Total;
                    result.RetainedStderr = stderr.Retained;
                    result.StderrTruncated = stderr.Truncated;
                    if (stderr.Error != null && result.CleanupError == null)
                        result.CleanupError = "stderr drain failed: " + stderr.Error.Message;
                }
            }
            catch (Exception error)
            {
                drainsComplete = false;
                if (result.CleanupError == null)
                    result.CleanupError = "drain completion failed: " + error.Message;
            }

            bool jobClosed = true;
            if (job != IntPtr.Zero)
            {
                jobClosed = CloseHandle(job);
                job = IntPtr.Zero;
                if (!jobClosed && result.CleanupError == null)
                    result.CleanupError = "CloseHandle(job) failed: " + Marshal.GetLastWin32Error();
            }

            bool reaped = process == null || !result.Started;
            if (process != null)
            {
                try
                {
                    if (result.Started)
                        reaped = process.HasExited;
                    process.Dispose();
                }
                catch (Exception error)
                {
                    if (result.CleanupError == null)
                        result.CleanupError = "process cleanup failed: " + error.Message;
                }
            }

            if (captureMode && exitedBeforeStop && result.LaunchError == null)
                result.Outcome = result.TotalStdoutBytes > 0 ? "natural_exit_after_data" : "natural_exit_before_data";

            result.CleanupComplete = drainsComplete && jobClosed && reaped && result.CleanupError == null;
            result.FinishedAtUtc = DateTime.UtcNow;
            result.DurationMilliseconds = clock.ElapsedMilliseconds;
            clock.Stop();
        }

        return result;
    }

    public static CaptureProbeResult Run(
        string fileName,
        string[] arguments,
        int timeoutMilliseconds,
        bool retainStdout,
        int killGraceMilliseconds)
    {
        return RunCore(fileName, arguments, timeoutMilliseconds, retainStdout, false, 0, 0, 0, killGraceMilliseconds);
    }

    public static CaptureProbeResult RunCapture(
        string fileName,
        string[] arguments,
        int firstDataTimeoutMilliseconds,
        int captureAfterFirstDataMilliseconds,
        int quitGraceMilliseconds,
        int killGraceMilliseconds)
    {
        return RunCore(
            fileName,
            arguments,
            firstDataTimeoutMilliseconds + captureAfterFirstDataMilliseconds + quitGraceMilliseconds + killGraceMilliseconds,
            false,
            true,
            firstDataTimeoutMilliseconds,
            captureAfterFirstDataMilliseconds,
            quitGraceMilliseconds,
            killGraceMilliseconds);
    }
}
