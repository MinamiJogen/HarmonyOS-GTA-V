import { common } from '@kit.AbilityKit';
import { fileIo, fileUri, picker } from '@kit.CoreFileKit';

export class GamePackageDirectory {
  uri: string;
  path: string;
  displayName: string;

  constructor(uri: string, path: string) {
    this.uri = uri;
    this.path = path;
    this.displayName = `Download/${path.slice(path.lastIndexOf('/') + 1)}`;
  }
}

export class DownloadGamePackage {
  name: string;
  path: string;
  bytes: number;

  constructor(name: string, path: string, bytes: number) {
    this.name = name;
    this.path = path;
    this.bytes = bytes;
  }
}

let directoryRequest: Promise<GamePackageDirectory> | undefined = undefined;

async function createDirectory(context: common.UIAbilityContext): Promise<GamePackageDirectory> {
  // DOWNLOAD creates the system-managed app directory and grants persistent
  // access. Its actual name comes from the system, not a hard-coded mount path.
  const documentPicker = new picker.DocumentViewPicker(context);
  const result: string[] = await documentPicker.save({ pickerMode: picker.DocumentPickerMode.DOWNLOAD });
  if (result.length === 0 || !result[0].startsWith('file://')) {
    throw new Error('未取得公共下载目录，可使用“选择游戏 ZIP”导入。');
  }
  const uri: fileUri.FileUri = new fileUri.FileUri(result[0]);
  const path: string = uri.path.replace(/\/+$/, '');
  if (uri.scheme !== 'file' || !path.startsWith('/') || path.indexOf('\0') >= 0 ||
    path.split('/').some((part: string) => part === '.' || part === '..')) {
    throw new Error('系统返回的下载目录无效。');
  }
  if (!await fileIo.access(path, fileIo.AccessModeType.READ_WRITE) || !(await fileIo.stat(path)).isDirectory()) {
    throw new Error('公共下载目录不可读写，可使用“选择游戏 ZIP”导入。');
  }
  const directory: GamePackageDirectory = new GamePackageDirectory(result[0], path);
  const helpPath: string = `${path}/如何导入游戏包.txt`;
  let exists: boolean = false;
  try { exists = await fileIo.access(helpPath); } catch (_) { exists = false; }
  if (!exists) {
    const help: fileIo.File = await fileIo.open(helpPath,
      fileIo.OpenMode.WRITE_ONLY | fileIo.OpenMode.CREATE | fileIo.OpenMode.APPEND);
    try {
      await fileIo.write(help.fd,
        'GTA V 游戏资源导入\n\n' +
        '1. 用文件管理将自己的兼容游戏 ZIP 复制到本目录，无需解压。\n' +
        '2. 返回 GTA V，点击刷新，再点击 ZIP 文件导入。也可在应用内选择其他位置的 ZIP。\n' +
        '3. 等待解压和校验完成，再点击开始游戏。导入不会改动存档，原 ZIP 会保留。\n\n' +
        '可使用包含 game/data 和 game/b/8b0b5899ed/shaders 的本地备份包，或 data/b 布局的兼容资源包。\n' +
        '公开应用壳只提供导入界面；实际运行需自行编译并签名的完整游戏安装包。\n');
    } finally {
      await fileIo.close(help);
    }
  }
  return directory;
}

export function getGamePackageDirectory(context: common.UIAbilityContext): Promise<GamePackageDirectory> {
  if (directoryRequest) { return directoryRequest; }
  const pending: Promise<GamePackageDirectory> = createDirectory(context).finally((): void => {
    directoryRequest = undefined;
  });
  directoryRequest = pending;
  return pending;
}

export async function listGamePackages(directory: GamePackageDirectory): Promise<DownloadGamePackage[]> {
  const names: string[] = await fileIo.listFile(directory.path);
  const packages: DownloadGamePackage[] = [];
  for (const name of names) {
    if (!name.toLowerCase().endsWith('.zip') || name.indexOf('/') >= 0 || name.indexOf('\\') >= 0) { continue; }
    const path: string = `${directory.path}/${name}`;
    try {
      const stat: fileIo.Stat = await fileIo.stat(path);
      if (stat.isFile() && stat.size > 0) {
        packages.push(new DownloadGamePackage(name, path, stat.size));
      }
    } catch (_) {
      // A file can disappear while the user manages the public directory.
    }
  }
  packages.sort((left: DownloadGamePackage, right: DownloadGamePackage) => left.name.localeCompare(right.name));
  return packages;
}

export async function openGamePackageDirectory(context: common.UIAbilityContext,
  directory: GamePackageDirectory): Promise<void> {
  await context.openLink('filemanager://openDirectory', {
    appLinkingOnly: false, parameters: { fileUri: directory.uri }
  });
}

export async function selectGamePackage(context: common.UIAbilityContext, directoryUri: string): Promise<string> {
  const documentPicker = new picker.DocumentViewPicker(context);
  const options: picker.DocumentSelectOptions = {
    maxSelectNumber: 1,
    fileSuffixFilters: ['ZIP 游戏包|.zip']
  };
  if (directoryUri.length > 0) { options.defaultFilePathUri = directoryUri; }
  const selected: string[] = await documentPicker.select(options);
  return selected.length > 0 ? selected[0] : '';
}
