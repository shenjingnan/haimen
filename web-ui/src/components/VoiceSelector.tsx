import Combobox from '@/components/Combobox';
import type { TtsVoice } from '@/types';

interface VoiceSelectorProps {
  voices: TtsVoice[];
  selectedVoice: string | null;
  onChange: (voiceId: string) => void;
}

export default function VoiceSelector({ voices, selectedVoice, onChange }: VoiceSelectorProps) {
  const options = voices.map((v) => ({
    value: v.id,
    label: `${v.name} (${v.id})`,
  }));

  return (
    <Combobox
      options={options}
      value={selectedVoice}
      onChange={onChange}
      placeholder="选择音色..."
      searchPlaceholder="搜索音色..."
      emptyText="未找到匹配音色"
      showCount
    />
  );
}
