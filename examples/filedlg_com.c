/* The Vista file dialogs, through COM: IFileOpenDialog / IFileSaveDialog.
 *
 *   x86_64-w64-mingw32-gcc -O2 -o filedlg_com.exe filedlg_com.c -lole32 -luuid -loleaut32
 *   nle filedlg_com.exe open C:\Users
 *   nle filedlg_com.exe save C:\Users
 *   nle filedlg_com.exe multi C:\Users
 */
#define COBJMACROS
#include <windows.h>
#include <shobjidl.h>
#include <stdio.h>
#include <string.h>

static void show_item(const char *tag, IShellItem *item) {
    PWSTR path = NULL;
    if (SUCCEEDED(IShellItem_GetDisplayName(item, SIGDN_FILESYSPATH, &path))) {
        printf("%s=%ls\n", tag, path);
        CoTaskMemFree(path);
    }
}

int main(int argc, char **argv) {
    const char *mode = argc > 1 ? argv[1] : "open";
    int save = !strcmp(mode, "save");
    CoInitializeEx(NULL, COINIT_APARTMENTTHREADED);
    IFileDialog *dlg = NULL;
    HRESULT hr = CoCreateInstance(save ? &CLSID_FileSaveDialog : &CLSID_FileOpenDialog, NULL, CLSCTX_INPROC_SERVER,
                                  save ? &IID_IFileSaveDialog : &IID_IFileOpenDialog, (void **)&dlg);
    if (FAILED(hr)) {
        printf("create failed %lx\n", hr);
        return 2;
    }
    COMDLG_FILTERSPEC types[] = {{L"Text files", L"*.txt"}, {L"All files", L"*.*"}};
    IFileDialog_SetFileTypes(dlg, 2, types);
    IFileDialog_SetDefaultExtension(dlg, L"txt");
    IFileDialog_SetTitle(dlg, L"Pick something");
    DWORD opts = 0;
    IFileDialog_GetOptions(dlg, &opts);
    if (!strcmp(mode, "multi")) opts |= FOS_ALLOWMULTISELECT;
    IFileDialog_SetOptions(dlg, opts | FOS_FORCEFILESYSTEM);
    if (argc > 2) {
        WCHAR dir[512];
        MultiByteToWideChar(CP_ACP, 0, argv[2], -1, dir, 512);
        IShellItem *folder = NULL;
        if (SUCCEEDED(SHCreateItemFromParsingName(dir, NULL, &IID_IShellItem, (void **)&folder))) {
            IFileDialog_SetFolder(dlg, folder);
            IShellItem_Release(folder);
        }
    }
    hr = IFileDialog_Show(dlg, NULL);
    if (FAILED(hr)) {
        printf("cancelled %s\n", hr == HRESULT_FROM_WIN32(ERROR_CANCELLED) ? "ok" : "other");
        return 1;
    }
    if (!strcmp(mode, "multi")) {
        IFileOpenDialog *open = NULL;
        IFileDialog_QueryInterface(dlg, &IID_IFileOpenDialog, (void **)&open);
        IShellItemArray *items = NULL;
        IFileOpenDialog_GetResults(open, &items);
        DWORD n = 0;
        IShellItemArray_GetCount(items, &n);
        printf("count=%lu\n", n);
        for (DWORD i = 0; i < n; i++) {
            IShellItem *it = NULL;
            IShellItemArray_GetItemAt(items, i, &it);
            show_item("file", it);
            IShellItem_Release(it);
        }
        IShellItemArray_Release(items);
        IFileOpenDialog_Release(open);
    } else {
        IShellItem *item = NULL;
        IFileDialog_GetResult(dlg, &item);
        show_item("path", item);
        PWSTR name = NULL;
        IShellItem_GetDisplayName(item, SIGDN_NORMALDISPLAY, &name);
        printf("name=%ls\n", name);
        CoTaskMemFree(name);
        UINT index = 0;
        IFileDialog_GetFileTypeIndex(dlg, &index);
        printf("filter=%u\n", index);
        IShellItem_Release(item);
    }
    IFileDialog_Release(dlg);
    CoUninitialize();
    return 0;
}
