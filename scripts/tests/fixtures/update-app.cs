using System;
using System.Diagnostics;
using System.IO;
using System.Reflection;
using System.Threading;
[assembly: AssemblyFileVersion("__VERSION__")]
class UpdateFixtureApp {
    static void Main(string[] args) {
        string directory = Path.GetDirectoryName(Process.GetCurrentProcess().MainModule.FileName);
        if (args.Length > 0 && args[0] == "--parent") {
            File.WriteAllText(Path.Combine(directory, "parent-started"), "ready");
            for (int i = 0; i < 300 && !File.Exists(Path.Combine(directory, "parent-exit")); i++) Thread.Sleep(100);
            return;
        }
        File.AppendAllText(Path.Combine(directory, "launches.txt"), "__VERSION__\n");
        for (int i = 0; i < 100 && File.Exists(Path.Combine(directory, "hold-open")); i++) Thread.Sleep(100);
    }
}
