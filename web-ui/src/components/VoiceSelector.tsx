import { useMemo } from 'react';

import Combobox from '@/components/Combobox';
import type { TtsVoice } from '@/types';

interface VoiceSelectorProps {
  voices: TtsVoice[];
  selectedVoice: string | null;
  onChange: (voiceId: string) => void;
}

/** 模型显示名 */
const MODEL_LABEL: Record<string, string> = {
  'seed-tts-2.0': '豆包 TTS 2.0（seed-tts-2.0）',
  'seed-tts-1.0': '豆包 TTS 1.0（seed-tts-1.0）',
};

export default function VoiceSelector({ voices, selectedVoice, onChange }: VoiceSelectorProps) {
  // 按音色所属模型分组展示（豆包音色带 model 字段；其他提供商无则归入「其他」）
  const groups = useMemo(() => {
    const byModel = new Map<string, TtsVoice[]>();
    for (const v of voices) {
      const model = v.model ?? '其他';
      const list = byModel.get(model);
      if (list) {
        list.push(v);
      } else {
        byModel.set(model, [v]);
      }
    }
    return [...byModel.entries()].map(([model, list]) => ({
      label: MODEL_LABEL[model] ?? model,
      options: list.map((v) => ({
        value: v.id,
        label: `${v.name} (${v.id})`,
      })),
    }));
  }, [voices]);

  return (
    <Combobox
      groups={groups}
      value={selectedVoice}
      onChange={onChange}
      placeholder="选择音色（将自动匹配对应模型）..."
      searchPlaceholder="搜索音色..."
      emptyText="未找到匹配音色"
      showCount
    />
  );
}
