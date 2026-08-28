using System;
using System.Diagnostics;
using System.IO;
class UpdateFixtureInstaller {
    static int Main(string[] args) {
        string directory = Path.GetDirectoryName(Process.GetCurrentProcess().MainModule.FileName);
        File.WriteAllLines(Path.Combine(directory, "received-args.txt"), args);
        string target = null;
        foreach (string arg in args) if (arg.StartsWith("/DIR=")) target = arg.Substring(5).Trim('"');
        if (target == null || !File.Exists(Path.Combine(target, "fixture-only.txt"))) return 91;
        string scenario = File.ReadAllText(Path.Combine(directory, "scenario.txt"));
        if (scenario == "failure") return 5;
        if (scenario == "unchanged") return 0;
        File.Copy(Path.Combine(directory, "payload.exe"), Path.Combine(target, "ElunviCanvas.exe"), true);
        if (scenario == "auto-launch") {
            Process.Start(new ProcessStartInfo(Path.Combine(target, "ElunviCanvas.exe")) { UseShellExecute = false });
        }
        return 0;
    }
}
