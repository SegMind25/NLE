/* The common file dialogs: open, save, and open with several files.
 *
 *   x86_64-w64-mingw32-gcc -O2 -o filedlg.exe filedlg.c -lcomdlg32
 *   nle filedlg.exe open C:\Users
 *   nle filedlg.exe save C:\Users
 *   nle filedlg.exe multi C:\Users
 *   nle filedlg.exe save C:\Users preset.txt   (a name already in the box)
 */
#include <windows.h>
#include <commdlg.h>
#include <stdio.h>
#include <string.h>

int main(int argc, char **argv) {
    char file[1024] = "";
    if (argc > 3) strncpy(file, argv[3], sizeof file - 1);
    const char *mode = argc > 1 ? argv[1] : "open";
    OPENFILENAMEA ofn;
    memset(&ofn, 0, sizeof ofn);
    ofn.lStructSize = sizeof ofn;
    ofn.lpstrFilter = "Text files (*.txt)\0*.txt\0All files (*.*)\0*.*\0";
    ofn.lpstrFile = file;
    ofn.nMaxFile = sizeof file;
    ofn.lpstrInitialDir = argc > 2 ? argv[2] : NULL;
    ofn.lpstrDefExt = "txt";
    BOOL ok;
    if (!strcmp(mode, "save")) {
        ofn.Flags = OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST;
        ok = GetSaveFileNameA(&ofn);
    } else if (!strcmp(mode, "multi")) {
        ofn.Flags = OFN_ALLOWMULTISELECT | OFN_EXPLORER | OFN_FILEMUSTEXIST;
        ok = GetOpenFileNameA(&ofn);
    } else {
        ofn.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST;
        ok = GetOpenFileNameA(&ofn);
    }
    if (!ok) {
        printf("cancelled %lx\n", CommDlgExtendedError());
        return 1;
    }
    if (!strcmp(mode, "multi") && file[ofn.nFileOffset - 1] == 0) {
        printf("folder=%s\n", file);
        for (const char *p = file + ofn.nFileOffset; *p; p += strlen(p) + 1) printf("file=%s\n", p);
        return 0;
    }
    printf("path=%s name=%s ext=%s filter=%lu\n", file, file + ofn.nFileOffset, ofn.nFileExtension ? file + ofn.nFileExtension : "-", ofn.nFilterIndex);
    return 0;
}
