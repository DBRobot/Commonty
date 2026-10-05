# What Windows does with its downloads, as far as wine can say: dd.exe
# runs, and the installer, run silently as a person would, puts the app
# and its engine's dll in their AppData. The window itself needs WebView2,
# which wine does not have; that is a real Windows' to show.
{
  pkgs,
  installer,
  dd,
}:
pkgs.runCommand "commonty-windows-installs"
  {
    nativeBuildInputs = [
      pkgs.wineWow64Packages.stable
      pkgs.xvfb-run
    ];
  }
  ''
    export HOME=$TMPDIR WINEPREFIX=$TMPDIR/wine WINEDEBUG=-all
    wine ${dd}/dd.exe --help > help.txt 2>&1 || true
    grep -q "the commonty client" help.txt || { cat help.txt; exit 1; }
    # WebView2 as Windows 10 and 11 have it, so the installer does not go
    # to fetch it (there is no network here)
    for k in 'HKLM\SOFTWARE\WOW6432Node' 'HKLM\SOFTWARE'; do
      wine reg add "$k\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}" /v pv /t REG_SZ /d 130.0.0.0 /f
    done
    # the installer's own exit is not the verdict (wine's display going
    # away as it ends can colour it); what it left on disk is
    xvfb-run -a wine ${installer}/commonty-setup.exe /S || echo "installer exited $?"
    wineserver -w || true
    dir=$(find $WINEPREFIX/drive_c/users -maxdepth 4 -type d -path "*AppData/Local/Commonty" | head -1)
    [ -n "$dir" ] || { echo "no Commonty in AppData:"; find $WINEPREFIX/drive_c/users -maxdepth 4; exit 1; }
    for f in commonty.exe commontynet.dll uninstall.exe; do
      [ -s "$dir/$f" ] || { echo "the installer left out $f"; exit 1; }
    done
    mkdir $out
    ls -l "$dir" > $out/installed
  ''
