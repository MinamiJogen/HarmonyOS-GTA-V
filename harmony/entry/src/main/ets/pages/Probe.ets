import probe, { ProbeResult, EmbeddedCaseResult } from 'libharmony_probe.so';
import { common } from '@kit.AbilityKit';
import { fileIo } from '@kit.CoreFileKit';

interface ProbeEvidence {
  capturedAt: string;
  result: ProbeResult;
}

interface VulkanVersion {
  major: number;
  minor: number;
  patch: number;
}
interface VulkanFeatures {
  textureCompressionBC: boolean;
  textureCompressionASTC_LDR: boolean;
}
interface VulkanDevice {
  name: string;
  api_version: VulkanVersion;
  core_features: VulkanFeatures;
}
interface VulkanSurface {
  extension_advertised: boolean;
  presentation_verified: boolean;
}
interface VulkanError {
  stage: string;
  message: string;
}
interface VulkanReport {
  status: string;
  physical_device_count: number;
  physical_devices: VulkanDevice[];
  ohos_surface: VulkanSurface;
  errors: VulkanError[];
}

@Entry
@Component
struct Probe {
  @State private cppStatus: string = '等待运行';
  @State private rustStatus: string = '等待运行';
  @State private runtimeStatus: string = '等待运行';
  @State private aotStatus: string = '尚未执行';
  @State private details: string = '';
  @State private embeddedStatus: string = '尚未执行';
  @State private embeddedDetails: string = '';
  @State private artifactPath: string = '';
  @State private vulkanStatus: string = '等待运行';
  @State private vulkanDetails: string = '';
  @State private evidenceStatus: string = '';

  aboutToAppear(): void {
    this.runProbe();
  }

  private runProbe(): void {
    this.cppStatus = '验证中';
    this.rustStatus = '尚未验证';
    this.runtimeStatus = '尚未验证';
    this.aotStatus = '尚未验证';
    this.embeddedStatus = '尚未验证';
    this.details = '';
    this.vulkanStatus = '验证中';
    this.vulkanDetails = '';
    try {
      const context = this.getUIContext().getHostContext() as common.UIAbilityContext;
      const reportPath: string = context ? `${context.filesDir}/vulkan-report.json` : '';
      const result: ProbeResult = probe.runProbe(this.artifactPath.trim(), reportPath);
      if (context) {
        const evidencePath: string = `${context.filesDir}/probe-result.json`;
        try {
          const evidence: ProbeEvidence = { capturedAt: new Date().toISOString(), result: result };
          const file = fileIo.openSync(evidencePath, fileIo.OpenMode.READ_WRITE | fileIo.OpenMode.CREATE | fileIo.OpenMode.TRUNC);
          try {
            fileIo.writeSync(file.fd, JSON.stringify(evidence));
          } finally {
            fileIo.closeSync(file);
          }
          this.evidenceStatus = `结果：${evidencePath}`;
        } catch (error) {
          this.evidenceStatus = `结果导出失败：${String(error)}`;
        }
      }
      const vulkan: VulkanReport = JSON.parse(result.vulkanJson) as VulkanReport;
      this.vulkanStatus = vulkan.status === 'pass' ? `Vulkan 枚举通过：${vulkan.physical_device_count} 个设备` : `Vulkan 枚举：${vulkan.status}`;
      let vulkanLines: string[] = [];
      vulkan.physical_devices.forEach((device: VulkanDevice) => {
        vulkanLines.push(`${device.name} · Vulkan ${device.api_version.major}.${device.api_version.minor}.${device.api_version.patch}`);
        vulkanLines.push(`BC: ${device.core_features.textureCompressionBC} · ASTC: ${device.core_features.textureCompressionASTC_LDR}`);
      });
      if (vulkan.ohos_surface) {
        vulkanLines.push(`VK_OHOS_surface: ${vulkan.ohos_surface.extension_advertised}`);
      }
      vulkan.errors.forEach((error: VulkanError) => { vulkanLines.push(`${error.stage}: ${error.message}`); });
      vulkanLines.push('当前仅枚举能力，尚未验证画面显示。');
      vulkanLines.push(result.vulkanReportSaved ? `报告：${result.vulkanReportPath}` : '完整报告见 hilog VULKAN_JSON 分片。');
      this.vulkanDetails = vulkanLines.join('\n');
      this.cppStatus = result.cppReady ? '通过：ArkTS → C++ Node-API' : '失败';
      this.rustStatus = result.rustLinked ?
        (result.rustResult === 42 ? '通过：Rust FFI 20 + 22 = 42' : `失败：返回 ${result.rustResult}`) : '未链接 Rust';
      this.runtimeStatus = result.runtimeAvailable ? 'Wasmtime runtime 已编译' : 'Wasmtime runtime 未编译';
      this.aotStatus = result.aotAttempted ?
        (result.aotPassed ? `通过：AOT add = ${result.aotResult}` : `失败：status ${result.aotStatus}`) : '尚未执行 AOT 模块';
      this.embeddedStatus = result.embeddedAttempted ?
        (result.embeddedPassed ? `通过：签名映射 AOT add = ${result.embeddedResult}` : `失败：签名映射 AOT status ${result.embeddedStatus}`) :
        (result.embeddedAvailable ? 'AOT 对象已链接，runtime 尚不可用' : '尚未链接 AOT 对象');
      this.embeddedDetails = result.embeddedCases.map((item: EmbeddedCaseResult) => {
        if (!item.available) {
          return `${item.name}: 未链接`;
        }
        if (!item.attempted) {
          return `${item.name}: 未执行`;
        }
        return item.passed ? `${item.name}: 通过 (${item.result})` : `${item.name}: 失败 (${item.status}) ${item.error}`;
      }).join('\n');
      this.details = `ABI: ${result.abi}\nRust: ${result.rustVersion}\n${result.error}\n${result.embeddedError}`;
    } catch (error) {
      this.cppStatus = 'Native 调用失败';
      this.details = String(error);
    }
  }

  build() {
    Scroll() {
      Column({ space: 14 }) {
        Text('HarmonyOS 运行时验证')
          .fontSize(25).fontWeight(FontWeight.Bold).fontColor('#EFF5FF')
          .width('100%')
        Text('API 26 · arm64 · 原型测试')
          .fontSize(14).fontColor('#A8BDD1').width('100%')
        Text(this.cppStatus).fontColor('#EFF5FF').width('100%')
        Text(this.rustStatus).fontColor('#EFF5FF').width('100%')
        Text(this.runtimeStatus).fontColor('#EFF5FF').width('100%')
        Text(this.aotStatus).fontColor('#EFF5FF').width('100%')
        Text(this.embeddedStatus).fontColor('#EFF5FF').width('100%')
        Text(this.embeddedDetails).fontSize(12).fontColor('#A8BDD1').width('100%')
        Text(this.vulkanStatus).fontColor('#EFF5FF').width('100%')
        Text(this.vulkanDetails).fontSize(12).fontColor('#A8BDD1').width('100%').copyOption(CopyOptions.LocalDevice)
        Text(this.evidenceStatus).fontSize(12).fontColor('#A8BDD1').width('100%').copyOption(CopyOptions.LocalDevice)
        Text('AOT 文件路径（应用可读取的本地文件）')
          .fontSize(13).fontColor('#A8BDD1').width('100%')
        TextInput({ text: this.artifactPath, placeholder: '/data/storage/el2/base/files/add.cwasm' })
          .onChange((value: string) => { this.artifactPath = value; })
          .fontSize(12).backgroundColor('#ECF1F7').height(48)
        Button('运行验证')
          .width('100%').onClick(() => { this.runProbe(); })
        Text(this.details).fontSize(12).fontColor('#A8BDD1')
          .width('100%').copyOption(CopyOptions.LocalDevice)
        Text('此页面只验证 Native、Rust 和 AOT 调用链。游戏内容尚未接入。')
          .fontSize(12).fontColor('#A8BDD1').width('100%')
      }
      .width('100%').padding({ left: 24, right: 24, top: 48, bottom: 32 })
      .alignItems(HorizontalAlign.Start)
    }
    .width('100%').height('100%').backgroundColor('#101820')
  }
}
