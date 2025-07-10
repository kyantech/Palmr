import { useCallback, useEffect, useRef, useState } from "react";
import { IconPlayerPause, IconPlayerPlay, IconVolume, IconVolume3, IconVolumeOff } from "@tabler/icons-react";

import { Button } from "@/components/ui/button";
import { Slider } from "@/components/ui/slider";
import { formatTime } from "@/utils/format-time";
import WaveformVisualizer from "./waveform-visualizer";

interface CustomAudioPlayerProps {
  src: string;
}

export function CustomAudioPlayer({ src }: CustomAudioPlayerProps) {
  const [isPlaying, setIsPlaying] = useState(false);
  const [progress, setProgress] = useState(0);
  const [volume, setVolume] = useState(1);
  const [duration, setDuration] = useState(0);
  const [currentTime, setCurrentTime] = useState(0);
  const [audioData, setAudioData] = useState<Float32Array | null>(null);
  const [isLoading, setIsLoading] = useState(true);
  const audioRef = useRef<HTMLAudioElement>(null);
  const previousVolume = useRef(1);

  const loadAudioData = useCallback(async () => {
    try {
      const response = await fetch(src);
      const arrayBuffer = await response.arrayBuffer();
      const audioContext = new (window.AudioContext || (window as any).webkitAudioContext)();
      const audioBuffer = await audioContext.decodeAudioData(arrayBuffer);

      const channelData = audioBuffer.getChannelData(0);
      const points = 200;
      const blockSize = Math.floor(channelData.length / points);
      const downsampledData = new Float32Array(points);

      for (let i = 0; i < points; i++) {
        const blockStart = blockSize * i;
        let sum = 0;
        for (let j = 0; j < blockSize; j++) {
          sum += Math.abs(channelData[blockStart + j]);
        }
        downsampledData[i] = sum / blockSize;
      }

      setAudioData(downsampledData);
    } catch (error) {
      console.error("Failed to load audio data:", error);
      setAudioData(null);
    }
  }, [src]);

  useEffect(() => {
    loadAudioData();
  }, [src, loadAudioData]);

  useEffect(() => {
    const audio = audioRef.current;
    if (!audio) return;

    const updateProgress = () => {
      const currentProgress = (audio.currentTime / audio.duration) * 100;
      setProgress(currentProgress);
      setCurrentTime(audio.currentTime);
    };

    const handleLoadedMetadata = () => {
      setDuration(audio.duration);
      setIsLoading(false);
    };

    audio.addEventListener("timeupdate", updateProgress);
    audio.addEventListener("ended", () => setIsPlaying(false));
    audio.addEventListener("loadedmetadata", handleLoadedMetadata);

    return () => {
      audio.removeEventListener("timeupdate", updateProgress);
      audio.removeEventListener("ended", () => setIsPlaying(false));
      audio.removeEventListener("loadedmetadata", handleLoadedMetadata);
    };
  }, []);

  const togglePlayPause = () => {
    if (!audioRef.current) return;

    if (isPlaying) {
      audioRef.current.pause();
    } else {
      audioRef.current.play();
    }
    setIsPlaying(!isPlaying);
  };

  const handleSeek = ([percentage]: number[]) => {
    if (!audioRef.current) return;

    const time = (percentage / 100) * audioRef.current.duration;
    audioRef.current.currentTime = time;
    setProgress(percentage);
  };

  const handleVolumeChange = (value: number[]) => {
    const newVolume = value[0];
    if (!audioRef.current) return;

    audioRef.current.volume = newVolume;
    setVolume(newVolume);
    if (newVolume > 0) {
      previousVolume.current = newVolume;
    }
  };

  const toggleMute = () => {
    if (!audioRef.current) return;

    if (volume > 0) {
      handleVolumeChange([0]);
    } else {
      handleVolumeChange([previousVolume.current]);
    }
  };

  const VolumeIcon = volume === 0 ? IconVolumeOff : volume < 0.5 ? IconVolume3 : IconVolume;

  return (
    <div className="flex flex-col gap-2 w-full">
      <audio ref={audioRef} src={src} preload="metadata" />

      <WaveformVisualizer progress={progress} onSeek={handleSeek} audioData={audioData} isLoading={isLoading} />

      <div className="flex items-center gap-4">
        <Button variant="outline" size="icon" onClick={togglePlayPause} disabled={isLoading} className="h-8 w-8">
          {isPlaying ? <IconPlayerPause className="h-4 w-4" /> : <IconPlayerPlay className="h-4 w-4" />}
        </Button>

        <div className="text-sm text-muted-foreground space-x-1">
          <span>{formatTime(currentTime)}</span>
          <span>/</span>
          <span>{formatTime(duration)}</span>
        </div>

        <div className="flex items-center gap-2 ml-auto">
          <Button variant="outline" size="icon" onClick={toggleMute} className="h-8 w-8">
            <VolumeIcon className="h-4 w-4" />
          </Button>
          <Slider value={[volume]} max={1} step={0.01} onValueChange={handleVolumeChange} className="w-24" />
        </div>
      </div>
    </div>
  );
}
