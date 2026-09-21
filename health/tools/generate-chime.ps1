param(
    [string]$OutputPath = (Join-Path $PSScriptRoot '..\assets\two-chairs.wav')
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$sampleRate = 44100
$amplitude = 0.24
$fadeSamples = [int]($sampleRate * 0.012)
$segments = @(
    @{ Frequency = 659.25; Duration = 0.145 },
    @{ Frequency = 0.0; Duration = 0.035 },
    @{ Frequency = 1046.50; Duration = 0.165 }
)
$samples = [System.Collections.Generic.List[int16]]::new()

foreach ($segment in $segments) {
    $count = [int]($sampleRate * $segment.Duration)
    for ($index = 0; $index -lt $count; $index++) {
        if ($segment.Frequency -eq 0.0) {
            $samples.Add(0)
            continue
        }
        $fadeIn = [Math]::Min(1.0, $index / [double]$fadeSamples)
        $fadeOut = [Math]::Min(1.0, ($count - 1 - $index) / [double]$fadeSamples)
        $envelope = [Math]::Min($fadeIn, $fadeOut)
        $angle = 2.0 * [Math]::PI * $segment.Frequency * $index / $sampleRate
        $value = [Math]::Sin($angle) * $amplitude * $envelope * [int16]::MaxValue
        $samples.Add([int16][Math]::Round($value))
    }
}

$directory = Split-Path -Parent $OutputPath
[System.IO.Directory]::CreateDirectory($directory) | Out-Null
$stream = [System.IO.File]::Open($OutputPath, [System.IO.FileMode]::Create)
try {
    $writer = [System.IO.BinaryWriter]::new($stream)
    try {
        $dataBytes = $samples.Count * 2
        $writer.Write([Text.Encoding]::ASCII.GetBytes('RIFF'))
        $writer.Write([int](36 + $dataBytes))
        $writer.Write([Text.Encoding]::ASCII.GetBytes('WAVE'))
        $writer.Write([Text.Encoding]::ASCII.GetBytes('fmt '))
        $writer.Write([int]16)
        $writer.Write([int16]1)
        $writer.Write([int16]1)
        $writer.Write([int]$sampleRate)
        $writer.Write([int]($sampleRate * 2))
        $writer.Write([int16]2)
        $writer.Write([int16]16)
        $writer.Write([Text.Encoding]::ASCII.GetBytes('data'))
        $writer.Write([int]$dataBytes)
        foreach ($sample in $samples) {
            $writer.Write($sample)
        }
    }
    finally {
        $writer.Dispose()
    }
}
finally {
    $stream.Dispose()
}

$duration = $samples.Count / [double]$sampleRate
if ($duration -ge 1.0) {
    throw "Generated chime must remain under one second; got $duration seconds"
}
Write-Output $OutputPath
