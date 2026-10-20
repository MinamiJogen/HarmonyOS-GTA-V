import { common } from '@kit.AbilityKit';
import { picker } from '@kit.CoreFileKit';

export async function selectGamePackage(context: common.UIAbilityContext): Promise<string> {
  const documentPicker = new picker.DocumentViewPicker(context);
  const options: picker.DocumentSelectOptions = {
    maxSelectNumber: 1,
    fileSuffixFilters: ['ZIP 游戏包|.zip']
  };
  const selected: string[] = await documentPicker.select(options);
  return selected.length > 0 ? selected[0] : '';
}

export async function selectSavePackage(context: common.UIAbilityContext): Promise<string> {
  const documentPicker = new picker.DocumentViewPicker(context);
  const options: picker.DocumentSelectOptions = { maxSelectNumber: 1, fileSuffixFilters: ['ZIP 存档包|.zip'] };
  const selected: string[] = await documentPicker.select(options);
  return selected.length > 0 ? selected[0] : '';
}
