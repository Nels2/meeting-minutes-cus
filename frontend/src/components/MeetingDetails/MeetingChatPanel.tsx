"use client";

import { FormEvent, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Bot, Loader2, Send, User } from 'lucide-react';
import { toast } from 'sonner';
import { Button } from '@/components/ui/button';
import { Textarea } from '@/components/ui/textarea';

interface MeetingChatMessage {
  role: 'user' | 'assistant';
  content: string;
}

interface MeetingChatPanelProps {
  meetingId: string;
  customContext?: string;
}

export function MeetingChatPanel({ meetingId, customContext }: MeetingChatPanelProps) {
  const [messages, setMessages] = useState<MeetingChatMessage[]>([]);
  const [question, setQuestion] = useState('');
  const [isSending, setIsSending] = useState(false);
  const inputRef = useRef<HTMLTextAreaElement | null>(null);

  const canSend = useMemo(() => question.trim().length > 0 && !isSending, [isSending, question]);

  const handleSubmit = async (event: FormEvent) => {
    event.preventDefault();
    if (!canSend) return;

    const nextQuestion = question.trim();
    const conversationHistory = messages.slice(-8);
    const nextMessages: MeetingChatMessage[] = [
      ...messages,
      { role: 'user', content: nextQuestion },
    ];

    setMessages(nextMessages);
    setQuestion('');
    setIsSending(true);

    try {
      const response = await invoke<{ answer: string }>('api_chat_with_meeting', {
        meetingId,
        question: nextQuestion,
        conversationHistory,
        customContext: customContext || null,
      });

      setMessages(current => [
        ...current,
        { role: 'assistant', content: response.answer },
      ]);
    } catch (error) {
      console.error('Meeting chat failed:', error);
      toast.error(error instanceof Error ? error.message : 'Failed to chat with meeting');
      setMessages(messages);
    } finally {
      setIsSending(false);
      inputRef.current?.focus();
    }
  };

  return (
    <aside className="hidden xl:flex w-80 border-l border-gray-200 dark:border-gray-800 bg-white dark:bg-gray-950 flex-col shrink-0">
      <div className="px-4 py-3 border-b border-gray-200 dark:border-gray-800">
        <h2 className="text-sm font-semibold text-gray-900 dark:text-gray-100">Chat</h2>
      </div>

      <div className="flex-1 overflow-y-auto p-4 space-y-4">
        {messages.length === 0 ? (
          <div className="text-sm text-gray-500 dark:text-gray-400">
            Ask about decisions, action items, owners, risks, or details from the transcript.
          </div>
        ) : (
          messages.map((message, index) => {
            const isUser = message.role === 'user';
            const Icon = isUser ? User : Bot;

            return (
              <div key={`${message.role}-${index}`} className="flex gap-2">
                <div className="mt-0.5 flex h-7 w-7 items-center justify-center rounded-full bg-gray-100 dark:bg-gray-900 text-gray-600 dark:text-gray-300 shrink-0">
                  <Icon className="h-4 w-4" />
                </div>
                <div className="min-w-0">
                  <div className="text-xs font-medium text-gray-500 dark:text-gray-400 mb-1">
                    {isUser ? 'You' : 'Meetily'}
                  </div>
                  <div className="text-sm leading-6 whitespace-pre-wrap text-gray-900 dark:text-gray-100">
                    {message.content}
                  </div>
                </div>
              </div>
            );
          })
        )}

        {isSending && (
          <div className="flex items-center gap-2 text-sm text-gray-500 dark:text-gray-400">
            <Loader2 className="h-4 w-4 animate-spin" />
            Thinking
          </div>
        )}
      </div>

      <form onSubmit={handleSubmit} className="border-t border-gray-200 dark:border-gray-800 p-3 space-y-2">
        <Textarea
          ref={inputRef}
          value={question}
          onChange={(event) => setQuestion(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter' && !event.shiftKey) {
              event.preventDefault();
              void handleSubmit(event as unknown as FormEvent);
            }
          }}
          placeholder="Ask about this meeting..."
          className="min-h-[84px] resize-none text-sm"
        />
        <Button type="submit" size="sm" className="w-full" disabled={!canSend}>
          {isSending ? <Loader2 className="h-4 w-4 animate-spin" /> : <Send className="h-4 w-4" />}
          Send
        </Button>
      </form>
    </aside>
  );
}
