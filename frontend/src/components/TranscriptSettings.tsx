import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from './ui/select';
import { Input } from './ui/input';
import { Button } from './ui/button';
import { Label } from './ui/label';
import { Eye, EyeOff, Lock, Unlock } from 'lucide-react';
import { toast } from 'sonner';
import { ModelManager } from './WhisperModelManager';
import { ParakeetModelManager } from './ParakeetModelManager';


export interface TranscriptModelProps {
    provider: 'localWhisper' | 'parakeet' | 'deepgram' | 'elevenLabs' | 'groq' | 'openai' | 'custom-openai';
    model: string;
    apiKey?: string | null;
}

export interface TranscriptSettingsProps {
    transcriptModelConfig: TranscriptModelProps;
    setTranscriptModelConfig: (config: TranscriptModelProps) => void;
    onModelSelect?: () => void;
}

export function TranscriptSettings({ transcriptModelConfig, setTranscriptModelConfig, onModelSelect }: TranscriptSettingsProps) {
    const [apiKey, setApiKey] = useState<string | null>(transcriptModelConfig.apiKey || null);
    const [showApiKey, setShowApiKey] = useState<boolean>(false);
    const [isApiKeyLocked, setIsApiKeyLocked] = useState<boolean>(true);
    const [isLockButtonVibrating, setIsLockButtonVibrating] = useState<boolean>(false);
    const [uiProvider, setUiProvider] = useState<TranscriptModelProps['provider']>(transcriptModelConfig.provider);
    const [selectedWhisperModel, setSelectedWhisperModel] = useState<string>(transcriptModelConfig.provider === 'localWhisper' ? transcriptModelConfig.model : 'small');
    const [selectedParakeetModel, setSelectedParakeetModel] = useState<string>(transcriptModelConfig.provider === 'parakeet' ? transcriptModelConfig.model : 'parakeet-tdt-0.6b-v3-int8');
    const [customOpenAIEndpoint, setCustomOpenAIEndpoint] = useState<string>('');

    // Sync uiProvider when backend config changes (e.g., after model selection or initial load)
    useEffect(() => {
        setUiProvider(transcriptModelConfig.provider);
    }, [transcriptModelConfig.provider]);

    useEffect(() => {
        if (transcriptModelConfig.provider === 'localWhisper') {
            setSelectedWhisperModel(transcriptModelConfig.model);
        }
        if (transcriptModelConfig.provider === 'parakeet') {
            setSelectedParakeetModel(transcriptModelConfig.model);
        }
    }, [transcriptModelConfig.provider, transcriptModelConfig.model]);

    useEffect(() => {
        if (transcriptModelConfig.provider === 'localWhisper' || transcriptModelConfig.provider === 'parakeet') {
            setApiKey(null);
        }
    }, [transcriptModelConfig.provider]);


    const fetchApiKey = async (provider: string) => {
        try {

            const data = await invoke('api_get_transcript_api_key', { provider }) as string;

            setApiKey(data || '');
        } catch (err) {
            console.error('Error fetching API key:', err);
            setApiKey(null);
        }
    };
    const fetchCustomOpenAIConfig = async () => {
        try {
            const config = await invoke('api_get_transcript_custom_openai_config') as any;
            if (config) {
                setCustomOpenAIEndpoint(config.endpoint || '');
                if (typeof config.apiKey !== 'undefined') {
                    setApiKey(config.apiKey || '');
                }
                const modelValue = transcriptModelConfig.model?.trim()
                    ? transcriptModelConfig.model
                    : (config.model || 'whisper-1');
                if (modelValue !== transcriptModelConfig.model) {
                    setTranscriptModelConfig({ ...transcriptModelConfig, model: modelValue });
                }
            }
        } catch (err) {
            console.error('Error fetching custom OpenAI config:', err);
        }
    };

    useEffect(() => {
        if (transcriptModelConfig.provider === 'custom-openai') {
            fetchCustomOpenAIConfig();
        }
    }, [transcriptModelConfig.provider]);
    const modelOptions = {
        localWhisper: [], // Model selection handled by ModelManager component
        parakeet: [], // Model selection handled by ParakeetModelManager component
        deepgram: ['nova-2-phonecall'],
        elevenLabs: ['eleven_multilingual_v2'],
        groq: ['llama-3.3-70b-versatile'],
        openai: ['gpt-4o'],
        'custom-openai': ['whisper-1'],
    };
    const isCustomOpenAI = uiProvider === 'custom-openai';
    const requiresApiKey = uiProvider === 'deepgram' || uiProvider === 'elevenLabs' || uiProvider === 'openai' || uiProvider === 'groq' || uiProvider === 'custom-openai';

    const handleInputClick = () => {
        if (isApiKeyLocked) {
            setIsLockButtonVibrating(true);
            setTimeout(() => setIsLockButtonVibrating(false), 500);
        }
    };

    const handleWhisperModelSelect = (modelName: string) => {
        // Always update config when model is selected, regardless of current provider
        // This ensures the model is set when user switches back
        setSelectedWhisperModel(modelName);
        setTranscriptModelConfig({
            ...transcriptModelConfig,
            provider: 'localWhisper', // Ensure provider is set correctly
            model: modelName
        });
        // Close modal after selection
        if (onModelSelect) {
            onModelSelect();
        }
    };

    const handleParakeetModelSelect = (modelName: string) => {
        // Always update config when model is selected, regardless of current provider
        // This ensures the model is set when user switches back
        setSelectedParakeetModel(modelName);
        setTranscriptModelConfig({
            ...transcriptModelConfig,
            provider: 'parakeet', // Ensure provider is set correctly
            model: modelName
        });
        // Close modal after selection
        if (onModelSelect) {
            onModelSelect();
        }
    };

    const canSaveCustomOpenAI = isCustomOpenAI
        && customOpenAIEndpoint.trim().length > 0
        && transcriptModelConfig.model.trim().length > 0;

    const saveCustomOpenAISettings = async () => {
        try {
            if (!canSaveCustomOpenAI) {
                toast.error('Please provide a valid endpoint and model.');
                return;
            }

            await invoke('api_save_transcript_custom_openai_config', {
                endpoint: customOpenAIEndpoint.trim(),
                apiKey: apiKey?.trim() || null,
                model: transcriptModelConfig.model.trim(),
                maxTokens: null,
                temperature: null,
                topP: null,
            });

            await invoke('api_save_transcript_config', {
                provider: 'custom-openai',
                model: transcriptModelConfig.model.trim(),
                apiKey: null,
            });

            toast.success('Custom OpenAI transcription settings saved.');
        } catch (err) {
            console.error('Failed to save custom OpenAI transcription settings:', err);
            toast.error('Failed to save Custom OpenAI settings.');
        }
    };

    return (
        <div>
            <div>
                {/* <div className="flex justify-between items-center mb-4">
                    <h3 className="text-lg font-semibold text-gray-900">Transcript Settings</h3>
                </div> */}
                <div className="space-y-4 pb-6">
                    <div>
                        <Label className="block text-sm font-medium text-gray-700 mb-1">
                            Transcript Model
                        </Label>
                        <div className="flex space-x-2 mx-1">
                            <Select
                                value={uiProvider}
                                onValueChange={(value) => {
                                    const provider = value as TranscriptModelProps['provider'];
                                    setUiProvider(provider);
                                    const newModel = provider === 'localWhisper'
                                        ? selectedWhisperModel
                                        : provider === 'parakeet'
                                            ? selectedParakeetModel
                                            : (modelOptions[provider]?.[0] || '');
                                    setTranscriptModelConfig({ ...transcriptModelConfig, provider, model: newModel });
                                    if (provider !== 'localWhisper' && provider !== 'parakeet') {
                                        fetchApiKey(provider);
                                    }
                                    if (provider === 'custom-openai') {
                                        fetchCustomOpenAIConfig();
                                    }
                                }}
                            >
                                <SelectTrigger className='focus:ring-1 focus:ring-blue-500 focus:border-blue-500'>
                                    <SelectValue placeholder="Select provider" />
                                </SelectTrigger>
                                <SelectContent>
                                    <SelectItem value="parakeet">⚡ Parakeet (Recommended - Real-time / Accurate)</SelectItem>
                                    <SelectItem value="localWhisper">🏠 Local Whisper (High Accuracy)</SelectItem>
                                    <SelectItem value="custom-openai">🌐 Custom OpenAI (Remote)</SelectItem>
                                    {/* <SelectItem value="deepgram">☁️ Deepgram (Backup)</SelectItem>
                                    <SelectItem value="elevenLabs">☁️ ElevenLabs</SelectItem>
                                    <SelectItem value="groq">☁️ Groq</SelectItem>
                                    <SelectItem value="openai">☁️ OpenAI</SelectItem> */}
                                </SelectContent>
                            </Select>

                            {uiProvider !== 'localWhisper' && uiProvider !== 'parakeet' && !isCustomOpenAI && (
                                <Select
                                    value={transcriptModelConfig.model}
                                    onValueChange={(value) => {
                                        const model = value as TranscriptModelProps['model'];
                                        setTranscriptModelConfig({ ...transcriptModelConfig, provider: uiProvider, model });
                                    }}
                                >
                                    <SelectTrigger className='focus:ring-1 focus:ring-blue-500 focus:border-blue-500'>
                                        <SelectValue placeholder="Select model" />
                                    </SelectTrigger>
                                    <SelectContent>
                                        {modelOptions[uiProvider].map((model) => (
                                            <SelectItem key={model} value={model}>{model}</SelectItem>
                                        ))}
                                    </SelectContent>
                                </Select>
                            )}

                        </div>
                    </div>

                    {isCustomOpenAI && (
                        <div className="space-y-4">
                            <div>
                                <Label className="block text-sm font-medium text-gray-700 mb-1">
                                    Custom OpenAI Endpoint
                                </Label>
                                <Input
                                    className="focus:ring-1 focus:ring-blue-500 focus:border-blue-500"
                                    value={customOpenAIEndpoint}
                                    onChange={(e) => setCustomOpenAIEndpoint(e.target.value)}
                                    placeholder="https://your-server/v1"
                                />
                            </div>
                            <div>
                                <Label className="block text-sm font-medium text-gray-700 mb-1">
                                    Model
                                </Label>
                                <Input
                                    className="focus:ring-1 focus:ring-blue-500 focus:border-blue-500"
                                    value={transcriptModelConfig.model}
                                    onChange={(e) => setTranscriptModelConfig({ ...transcriptModelConfig, model: e.target.value })}
                                    placeholder="whisper-1"
                                />
                            </div>
                        </div>
                    )}

                    {uiProvider === 'localWhisper' && (
                        <div className="mt-6">
                            <ModelManager
                                selectedModel={transcriptModelConfig.provider === 'localWhisper' ? transcriptModelConfig.model : undefined}
                                onModelSelect={handleWhisperModelSelect}
                                autoSave={true}
                            />
                        </div>
                    )}

                    {uiProvider === 'parakeet' && (
                        <div className="mt-6">
                            <ParakeetModelManager
                                selectedModel={transcriptModelConfig.provider === 'parakeet' ? transcriptModelConfig.model : undefined}
                                onModelSelect={handleParakeetModelSelect}
                                autoSave={true}
                            />
                        </div>
                    )}


                    {requiresApiKey && (
                        <div>
                            <Label className="block text-sm font-medium text-gray-700 mb-1">
                                API Key
                            </Label>
                            <div className="relative mx-1">
                                <Input
                                    type={showApiKey ? "text" : "password"}
                                    className={`pr-24 focus:ring-1 focus:ring-blue-500 focus:border-blue-500 ${isApiKeyLocked ? 'bg-gray-100 cursor-not-allowed' : ''
                                        }`}
                                    value={apiKey || ''}
                                    onChange={(e) => setApiKey(e.target.value)}
                                    disabled={isApiKeyLocked}
                                    onClick={handleInputClick}
                                    placeholder="Enter your API key"
                                />
                                {isApiKeyLocked && (
                                    <div
                                        onClick={handleInputClick}
                                        className="absolute inset-0 flex items-center justify-center bg-gray-100 bg-opacity-50 rounded-md cursor-not-allowed"
                                    />
                                )}
                                <div className="absolute inset-y-0 right-0 pr-1 flex items-center">
                                    <Button
                                        type="button"
                                        variant="ghost"
                                        size="icon"
                                        onClick={() => setIsApiKeyLocked(!isApiKeyLocked)}
                                        className={`transition-colors duration-200 ${isLockButtonVibrating ? 'animate-vibrate text-red-500' : ''
                                            }`}
                                        title={isApiKeyLocked ? "Unlock to edit" : "Lock to prevent editing"}
                                    >
                                        {isApiKeyLocked ? <Lock className="h-4 w-4" /> : <Unlock className="h-4 w-4" />}
                                    </Button>
                                    <Button
                                        type="button"
                                        variant="ghost"
                                        size="icon"
                                        onClick={() => setShowApiKey(!showApiKey)}
                                    >
                                        {showApiKey ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
                                    </Button>
                                </div>
                            </div>
                        </div>
                    )}

                    {isCustomOpenAI && (
                        <div className="pt-2">
                            <Button
                                type="button"
                                onClick={saveCustomOpenAISettings}
                                disabled={!canSaveCustomOpenAI}
                            >
                                Save Custom OpenAI Settings
                            </Button>
                        </div>
                    )}
                </div>
            </div>
        </div >
    )
}








