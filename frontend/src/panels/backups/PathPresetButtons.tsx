import type { BackupPathPreset } from "../../presets/backupPathPresets";

export function PathPresetButtons({
  onApply,
  presets,
}: {
  onApply: (preset: BackupPathPreset) => void;
  presets: BackupPathPreset[];
}) {
  return (
    <div className="pathPresetStrip" aria-label="Selected path presets">
      <span>Presets</span>
      {presets.map((preset) => (
        <button
          key={preset.label}
          onClick={() => onApply(preset)}
          title={preset.description}
          type="button"
        >
          {preset.label}
        </button>
      ))}
    </div>
  );
}
