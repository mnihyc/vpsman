import {
  presetPathsText,
  type BackupPathPreset,
} from "../../presets/backupPathPresets";

export function PathPresetButtons({
  onApply,
  presets,
}: {
  onApply: (value: string) => void;
  presets: BackupPathPreset[];
}) {
  return (
    <div className="pathPresetStrip" aria-label="Selected path presets">
      <span>Presets</span>
      {presets.map((preset) => (
        <button
          key={preset.label}
          onClick={() => onApply(presetPathsText(preset.paths))}
          title={preset.description}
          type="button"
        >
          {preset.label}
        </button>
      ))}
    </div>
  );
}
