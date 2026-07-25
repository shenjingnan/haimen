import { Check, ChevronsUpDown } from 'lucide-react';
import { useState } from 'react';

import { Button } from '@/components/ui/button';
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from '@/components/ui/command';
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover';
import { cn } from '@/lib/utils';
import type { TtsVoice } from '@/types';

interface VoiceSelectorProps {
  voices: TtsVoice[];
  selectedVoice: string | null;
  onChange: (voiceId: string) => void;
}

export default function VoiceSelector({ voices, selectedVoice, onChange }: VoiceSelectorProps) {
  const [open, setOpen] = useState(false);

  const selected = voices.find((v) => v.id === selectedVoice);

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="outline"
          role="combobox"
          aria-expanded={open}
          className="w-full justify-between"
        >
          {selected ? `${selected.name} (${selected.id})` : '选择音色...'}
          <ChevronsUpDown className="ml-2 h-4 w-4 shrink-0 opacity-50" />
        </Button>
      </PopoverTrigger>
      <PopoverContent className="w-[var(--radix-popover-trigger-width)] p-0">
        <Command>
          <CommandInput placeholder="搜索音色..." />
          <CommandEmpty>未找到匹配音色</CommandEmpty>
          <CommandList>
            <CommandGroup>
              {voices.map((voice) => (
                <CommandItem
                  key={voice.id}
                  value={voice.id}
                  onSelect={(currentValue) => {
                    onChange(currentValue);
                    setOpen(false);
                  }}
                >
                  <Check
                    className={cn(
                      'mr-2 h-4 w-4',
                      selectedVoice === voice.id ? 'opacity-100' : 'opacity-0',
                    )}
                  />
                  <div className="flex flex-1 flex-col">
                    <span>{voice.name}</span>
                    <span className="text-xs text-muted-foreground">
                      {voice.id} · {voice.language}
                    </span>
                  </div>
                </CommandItem>
              ))}
            </CommandGroup>
          </CommandList>
          <div className="border-t border-border px-3 py-1.5 text-xs text-muted-foreground">
            共 {voices.length} 个可用音色
          </div>
        </Command>
      </PopoverContent>
    </Popover>
  );
}
