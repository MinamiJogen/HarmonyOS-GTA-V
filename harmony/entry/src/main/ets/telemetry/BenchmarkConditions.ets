import { taskpool } from '@kit.ArkTS';
import { systemDateTime } from '@kit.BasicServicesKit';
import thermal from '@ohos.thermal';
import batteryInfo from '@ohos.batteryInfo';

export interface BenchmarkConditionSample {
  uptimeMs: number;
  completedUptimeMs: number;
  durationMs: number;
  thermalLevel: number | null;
  batteryTemperatureDeciC: number | null;
  batteryTemperatureC: number | null;
  batterySOC: number | null;
  pluggedType: number | null;
  chargingStatus: number | null;
  apiFailures: string[];
}

export interface BenchmarkConditionsSnapshot {
  enabled: boolean;
  active: boolean;
  inFlight: boolean;
  generation: number;
  sampleGeneration: number | null;
  fresh: boolean;
  sampleAgeMs: number | null;
  receivedUptimeMs: number | null;
  sample: BenchmarkConditionSample | null;
  temperatureBoundary: string;
}

@Concurrent
function collectBenchmarkConditions(): BenchmarkConditionSample {
  const started: number = systemDateTime.getUptime(systemDateTime.TimeType.STARTUP, false);
  const sample: BenchmarkConditionSample = {
    uptimeMs: started, completedUptimeMs: started, durationMs: 0,
    thermalLevel: null, batteryTemperatureDeciC: null, batteryTemperatureC: null,
    batterySOC: null, pluggedType: null, chargingStatus: null, apiFailures: []
  };
  // These synchronous getters can perform IPC. Keep every getter and syscap
  // query inside taskpool, including failure paths.
  let thermalAvailable: boolean = false;
  let batteryAvailable: boolean = false;
  try { thermalAvailable = canIUse('SystemCapability.PowerManager.ThermalManager'); }
  catch (error) { sample.apiFailures.push(`thermal.syscap: ${String(error)}`); }
  try { batteryAvailable = canIUse('SystemCapability.PowerManager.BatteryManager.Core'); }
  catch (error) { sample.apiFailures.push(`battery.syscap: ${String(error)}`); }
  if (thermalAvailable) {
    try {
      const value: number = thermal.getLevel();
      if (!Number.isInteger(value) || value < 0 || value > 7) { throw new Error('invalid thermal level'); }
      sample.thermalLevel = value;
    } catch (error) { sample.apiFailures.push(`thermal.getLevel: ${String(error)}`); }
  } else { sample.apiFailures.push('thermal: capability unavailable'); }
  if (batteryAvailable) {
    try {
      const value: number = batteryInfo.batteryTemperature;
      if (!Number.isFinite(value)) { throw new Error('invalid battery temperature'); }
      sample.batteryTemperatureDeciC = value;
      sample.batteryTemperatureC = value / 10;
    } catch (error) { sample.apiFailures.push(`batteryTemperature: ${String(error)}`); }
    try {
      const value: number = batteryInfo.batterySOC;
      if (!Number.isInteger(value) || value < 0 || value > 100) { throw new Error('invalid battery SOC'); }
      sample.batterySOC = value;
    } catch (error) { sample.apiFailures.push(`batterySOC: ${String(error)}`); }
    try {
      const value: number = batteryInfo.pluggedType;
      if (!Number.isInteger(value) || value < 0) { throw new Error('invalid plugged type'); }
      sample.pluggedType = value;
    } catch (error) { sample.apiFailures.push(`pluggedType: ${String(error)}`); }
    try {
      const value: number = batteryInfo.chargingStatus;
      if (!Number.isInteger(value) || value < 0) { throw new Error('invalid charging status'); }
      sample.chargingStatus = value;
    } catch (error) { sample.apiFailures.push(`chargingStatus: ${String(error)}`); }
  } else { sample.apiFailures.push('battery: capability unavailable'); }
  sample.completedUptimeMs = systemDateTime.getUptime(systemDateTime.TimeType.STARTUP, false);
  sample.durationMs = Math.max(0, sample.completedUptimeMs - started);
  return sample;
}

async function readBenchmarkConditions(): Promise<BenchmarkConditionSample> {
  // Convert a synchronous task submission failure into a rejected promise too.
  return await taskpool.execute(collectBenchmarkConditions) as BenchmarkConditionSample;
}

export function benchmarkMonitorEnabled(configText: string): boolean {
  const config: Record<string, Object> = JSON.parse(configText) as Record<string, Object>;
  return config !== null && typeof config === 'object' && !Array.isArray(config) &&
    config['native_benchmark_monitor'] === true;
}

// One instance survives foreground and page transitions so an old task cannot
// overlap its successor. A task cannot be cancelled once in IPC; its generation
// must still match before its result is published.
export class BenchmarkConditions {
  private enabled: boolean = false;
  private active: boolean = false;
  private timer: number = -1;
  private inFlight: boolean = false;
  private generation: number = 0;
  private sampleGeneration: number = -1;
  private receivedUptimeMs: number = -1;
  private sample: BenchmarkConditionSample | null = null;

  configure(enabled: boolean): void {
    if (this.enabled === enabled) { return; }
    this.enabled = enabled;
    this.generation += 1;
    if (!enabled) { this.setActive(false); }
  }

  setActive(requested: boolean): void {
    const active: boolean = this.enabled && requested;
    if (this.active === active) { return; }
    this.active = active;
    this.generation += 1;
    if (this.timer >= 0) { clearInterval(this.timer); this.timer = -1; }
    if (active) {
      this.collect();
      this.timer = setInterval(() => { this.collect(); }, 5000);
    }
  }

  stop(): void {
    this.configure(false);
    this.setActive(false);
  }

  snapshot(nowUptimeMs: number): BenchmarkConditionsSnapshot {
    const age: number | null = this.sample === null ? null :
      Math.max(0, nowUptimeMs - this.sample.uptimeMs);
    return {
      enabled: this.enabled, active: this.active, inFlight: this.inFlight,
      generation: this.generation,
      sampleGeneration: this.sampleGeneration < 0 ? null : this.sampleGeneration,
      fresh: this.enabled && this.active && this.sampleGeneration === this.generation &&
        age !== null && age <= 15000,
      sampleAgeMs: age, receivedUptimeMs: this.receivedUptimeMs < 0 ? null : this.receivedUptimeMs,
      sample: this.sample,
      temperatureBoundary: 'battery temperature only; thermal level is a device severity category; neither is GPU or CPU core temperature'
    };
  }

  private collect(): void {
    if (!this.enabled || !this.active || this.inFlight) { return; }
    const generation: number = this.generation;
    this.inFlight = true;
    readBenchmarkConditions().then((result: BenchmarkConditionSample) => {
      if (!this.active || !this.enabled || generation !== this.generation) { return; }
      this.sample = result;
      this.sampleGeneration = generation;
      this.receivedUptimeMs = systemDateTime.getUptime(systemDateTime.TimeType.STARTUP, false);
    }).catch((error: Error) => {
      if (!this.active || !this.enabled || generation !== this.generation) { return; }
      const now: number = systemDateTime.getUptime(systemDateTime.TimeType.STARTUP, false);
      this.sample = {
        uptimeMs: now, completedUptimeMs: now, durationMs: 0, thermalLevel: null,
        batteryTemperatureDeciC: null, batteryTemperatureC: null, batterySOC: null,
        pluggedType: null, chargingStatus: null, apiFailures: [`taskpool: ${String(error)}`]
      };
      this.sampleGeneration = generation;
      this.receivedUptimeMs = now;
    }).finally(() => {
      this.inFlight = false;
      // A resume while the old task was running skipped its initial request.
      if (this.active && this.enabled && generation !== this.generation) { this.collect(); }
    });
  }
}
