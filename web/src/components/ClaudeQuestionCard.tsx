import { useState } from "react";
import type { ClaudeQuestion } from "../utils/claudeToolInput";
import type { DecisionSubmitter } from "./ClaudeDialogCard";
import { PRIMARY_BUTTON_CLASS, TEXT_INPUT_CLASS } from "./claudeStyles";

interface QuestionAnswer {
  labels: string[];
  otherChosen: boolean;
  otherText: string;
}

const EMPTY_ANSWER: QuestionAnswer = { labels: [], otherChosen: false, otherText: "" };

/** The value Claude expects for one question: a label, comma-joined labels, or free text. */
function answerValue(answer: QuestionAnswer): string {
  const other = answer.otherChosen ? answer.otherText.trim() : "";
  return [...answer.labels, ...(other ? [other] : [])].join(", ");
}

export function ClaudeQuestionCard({
  questions,
  decision,
}: {
  questions: ClaudeQuestion[];
  decision: DecisionSubmitter;
}) {
  const [answers, setAnswers] = useState<QuestionAnswer[]>(() => questions.map(() => EMPTY_ANSWER));
  const complete = questions.every((_, index) => answerValue(answers[index] ?? EMPTY_ANSWER) !== "");

  const updateAnswer = (index: number, answer: QuestionAnswer) => {
    setAnswers((current) => current.map((existing, i) => (i === index ? answer : existing)));
  };

  const submit = () => {
    const result: Record<string, string> = {};
    questions.forEach((question, index) => {
      result[question.question] = answerValue(answers[index] ?? EMPTY_ANSWER);
    });
    decision.submit({ decision: "answer", answers: result });
  };

  return (
    <div className="flex flex-col gap-3">
      {questions.map((question, index) => (
        <QuestionField
          key={index}
          name={`q${index}`}
          question={question}
          answer={answers[index] ?? EMPTY_ANSWER}
          disabled={decision.pending}
          onChange={(answer) => updateAnswer(index, answer)}
        />
      ))}
      <div>
        <button
          className={PRIMARY_BUTTON_CLASS}
          disabled={!complete || decision.pending}
          onClick={submit}
        >
          Submit
        </button>
      </div>
    </div>
  );
}

function QuestionField({
  name,
  question,
  answer,
  disabled,
  onChange,
}: {
  name: string;
  question: ClaudeQuestion;
  answer: QuestionAnswer;
  disabled: boolean;
  onChange: (answer: QuestionAnswer) => void;
}) {
  const inputType = question.multiSelect ? "checkbox" : "radio";

  const toggleLabel = (label: string, checked: boolean) => {
    if (!question.multiSelect) {
      onChange({ ...answer, labels: [label], otherChosen: false });
      return;
    }
    const labels = checked
      ? [...answer.labels, label]
      : answer.labels.filter((existing) => existing !== label);
    onChange({ ...answer, labels });
  };

  const toggleOther = (checked: boolean) => {
    onChange(
      question.multiSelect
        ? { ...answer, otherChosen: checked }
        : { ...answer, labels: [], otherChosen: true },
    );
  };

  return (
    <fieldset className="flex min-w-0 flex-col gap-1.5" disabled={disabled}>
      <legend className="mb-1 flex flex-col gap-1 text-[12px] text-[var(--ok-text)]">
        {question.header && (
          <span className="w-fit rounded-[3px] bg-[var(--ok-header)] px-1.5 text-[10px] uppercase text-[var(--ok-text-secondary)]">
            {question.header}
          </span>
        )}
        <span>{question.question}</span>
      </legend>
      {question.options.map((option) => (
        <label key={option.label} className="flex cursor-pointer items-start gap-2 rounded-[3px] px-1 py-1 hover:bg-[var(--ok-hover)]">
          <input
            className="mt-0.5"
            type={inputType}
            name={name}
            checked={answer.labels.includes(option.label)}
            onChange={(event) => toggleLabel(option.label, event.target.checked)}
          />
          <span className="min-w-0">
            <span className="block text-[12px] text-[var(--ok-text)]">{option.label}</span>
            {option.description && (
              <span className="block text-[11px] text-[var(--ok-text-muted)]">{option.description}</span>
            )}
          </span>
        </label>
      ))}
      <label className="flex items-center gap-2 px-1 py-1">
        <input
          type={inputType}
          name={name}
          checked={answer.otherChosen}
          onChange={(event) => toggleOther(event.target.checked)}
        />
        <input
          className={TEXT_INPUT_CLASS}
          value={answer.otherText}
          placeholder="Other"
          aria-label="Other answer"
          onFocus={() => {
            if (!answer.otherChosen) toggleOther(true);
          }}
          onChange={(event) => onChange({ ...answer, otherText: event.target.value })}
        />
      </label>
    </fieldset>
  );
}
