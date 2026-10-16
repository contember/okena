/**
 * ClaudeDialogCard.tsx — one open Claude dialog with the controls to answer
 * it. The card kind follows `tool_name`: `AskUserQuestion` → questions,
 * `ExitPlanMode` → plan approval, anything else → allow / deny.
 */

import React, { useState } from 'react';
import {
  Pressable,
  ScrollView,
  StyleSheet,
  Text,
  TextInput,
  View,
} from 'react-native';

import {
  EMPTY_SELECTION,
  questionAnswers,
  toggleQuestionOption,
  type QuestionSelection,
} from '../models/claudeChat';
import {
  previewClaudeToolInput,
  readClaudePlan,
  readClaudeQuestions,
  type ApiClaudeDecision,
  type ApiClaudeDialog,
  type ClaudeQuestion,
} from '../native/claude';
import { OkenaColors, TerminalTheme } from '../theme';

export interface ClaudeDialogCardProps {
  dialog: ApiClaudeDialog;
  onDecide: (decision: ApiClaudeDecision) => void;
}

export const ClaudeDialogCard: React.FC<ClaudeDialogCardProps> = ({
  dialog,
  onDecide,
}) => {
  if (dialog.tool_name === 'AskUserQuestion') {
    const questions = readClaudeQuestions(dialog.tool_input);
    if (questions !== null)
      return <QuestionCard questions={questions} onDecide={onDecide} />;
  }
  if (dialog.tool_name === 'ExitPlanMode') {
    return (
      <PlanCard
        plan={readClaudePlan(dialog.tool_input)}
        onDecide={onDecide}
      />
    );
  }
  return <PermissionCard dialog={dialog} onDecide={onDecide} />;
};

// ── default tool: allow / always allow / deny ────────────────────────────────

const PermissionCard: React.FC<ClaudeDialogCardProps> = ({
  dialog,
  onDecide,
}) => {
  const [reason, setReason] = useState('');
  return (
    <View style={styles.card}>
      <Text style={styles.title}>{dialog.tool_name}</Text>
      <ScrollView style={styles.preview} nestedScrollEnabled>
        <Text style={styles.mono} selectable>
          {previewClaudeToolInput(dialog.tool_input)}
        </Text>
      </ScrollView>
      <ReasonInput
        value={reason}
        onChange={setReason}
        placeholder="Reason for denying (optional)"
      />
      <View style={styles.actions}>
        <CardButton
          label="Allow"
          primary
          onPress={() => onDecide({ decision: 'allow' })}
        />
        {dialog.permission_suggestions.length > 0 ? (
          <CardButton
            label="Always allow"
            onPress={() => onDecide({ decision: 'allow', always: true })}
          />
        ) : null}
        <CardButton
          label="Deny"
          danger
          onPress={() => onDecide(denial(reason))}
        />
      </View>
    </View>
  );
};

// ── ExitPlanMode: approve / keep planning ────────────────────────────────────

const PlanCard: React.FC<{
  // Without a plan file Claude sends no plan; it is then in the transcript.
  plan: string | undefined;
  onDecide: ClaudeDialogCardProps['onDecide'];
}> = ({ plan, onDecide }) => {
  const [feedback, setFeedback] = useState('');
  return (
    <View style={styles.card}>
      <Text style={styles.title}>Plan ready for review</Text>
      {plan !== undefined ? (
        <ScrollView style={styles.plan} nestedScrollEnabled>
          <Text style={styles.body} selectable>
            {plan}
          </Text>
        </ScrollView>
      ) : (
        <Text style={styles.body}>
          Claude wants to exit plan mode. The plan is in the transcript above.
        </Text>
      )}
      <ReasonInput
        value={feedback}
        onChange={setFeedback}
        placeholder="What to change (optional)"
      />
      <View style={styles.actions}>
        <CardButton
          label="Approve"
          primary
          onPress={() => onDecide({ decision: 'allow' })}
        />
        <CardButton
          label="Keep planning"
          onPress={() => onDecide(denial(feedback))}
        />
      </View>
    </View>
  );
};

// ── AskUserQuestion: options per question ────────────────────────────────────

const QuestionCard: React.FC<{
  questions: ClaudeQuestion[];
  onDecide: ClaudeDialogCardProps['onDecide'];
}> = ({ questions, onDecide }) => {
  const [selections, setSelections] = useState<
    Record<string, QuestionSelection>
  >({});
  const answers = questionAnswers(questions, selections);

  const update = (question: ClaudeQuestion, next: QuestionSelection) =>
    setSelections((current) => ({ ...current, [question.question]: next }));

  return (
    <View style={styles.card}>
      {questions.map((question, questionIndex) => {
        const selection = selections[question.question] ?? EMPTY_SELECTION;
        return (
          <View key={questionIndex} style={styles.question}>
            {question.header ? (
              <Text style={styles.header}>{question.header}</Text>
            ) : null}
            <Text style={styles.body}>{question.question}</Text>
            {question.options.map((option, optionIndex) => (
              <OptionRow
                key={optionIndex}
                label={option.label}
                description={option.description}
                multiSelect={question.multiSelect}
                selected={selection.labels.includes(option.label)}
                onPress={() =>
                  update(
                    question,
                    toggleQuestionOption(question, selection, option.label),
                  )
                }
              />
            ))}
            <OptionRow
              label="Other"
              multiSelect={question.multiSelect}
              selected={selection.other !== null}
              onPress={() =>
                update(
                  question,
                  toggleQuestionOption(question, selection, null),
                )
              }
            />
            {selection.other !== null ? (
              <TextInput
                style={styles.input}
                value={selection.other}
                onChangeText={(other) =>
                  update(question, { ...selection, other })
                }
                placeholder="Your answer"
                placeholderTextColor={OkenaColors.textTertiary}
              />
            ) : null}
          </View>
        );
      })}
      <View style={styles.actions}>
        <CardButton
          label="Submit"
          primary
          disabled={answers === null}
          onPress={() => {
            if (answers !== null) onDecide({ decision: 'answer', answers });
          }}
        />
      </View>
    </View>
  );
};

const OptionRow: React.FC<{
  label: string;
  description?: string;
  multiSelect: boolean;
  selected: boolean;
  onPress: () => void;
}> = ({ label, description, multiSelect, selected, onPress }) => (
  <Pressable
    style={styles.option}
    onPress={onPress}
    accessibilityRole={multiSelect ? 'checkbox' : 'radio'}
    accessibilityState={{ checked: selected }}
  >
    <Text style={[styles.mark, selected && styles.markSelected]}>
      {multiSelect ? (selected ? '☑' : '☐') : selected ? '◉' : '○'}
    </Text>
    <View style={styles.flex}>
      <Text style={styles.body}>{label}</Text>
      {description ? (
        <Text style={styles.description}>{description}</Text>
      ) : null}
    </View>
  </Pressable>
);

// ── shared bits ────────────────────────────────────────────────────────────

function denial(text: string): ApiClaudeDecision {
  const message = text.trim();
  return message.length > 0
    ? { decision: 'deny', message }
    : { decision: 'deny' };
}

const ReasonInput: React.FC<{
  value: string;
  onChange: (value: string) => void;
  placeholder: string;
}> = ({ value, onChange, placeholder }) => (
  <TextInput
    style={styles.input}
    value={value}
    onChangeText={onChange}
    placeholder={placeholder}
    placeholderTextColor={OkenaColors.textTertiary}
  />
);

const CardButton: React.FC<{
  label: string;
  onPress: () => void;
  primary?: boolean;
  danger?: boolean;
  disabled?: boolean;
}> = ({ label, onPress, primary, danger, disabled }) => (
  <Pressable
    style={[
      styles.button,
      primary && styles.buttonPrimary,
      danger && styles.buttonDanger,
      disabled && styles.buttonDisabled,
    ]}
    disabled={disabled}
    onPress={onPress}
    accessibilityRole="button"
  >
    <Text style={[styles.buttonText, primary && styles.buttonTextPrimary]}>
      {label}
    </Text>
  </Pressable>
);

const styles = StyleSheet.create({
  flex: { flex: 1 },
  card: {
    backgroundColor: OkenaColors.surfaceElevated,
    borderRadius: 10,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.warning,
    padding: 12,
    marginBottom: 8,
  },
  title: {
    color: OkenaColors.textPrimary,
    fontSize: 14,
    fontWeight: '600',
    marginBottom: 6,
  },
  header: {
    color: OkenaColors.accent,
    fontSize: 11,
    fontWeight: '600',
    marginBottom: 2,
  },
  body: { color: OkenaColors.textPrimary, fontSize: 14 },
  description: { color: OkenaColors.textSecondary, fontSize: 12, marginTop: 1 },
  mono: {
    color: OkenaColors.textPrimary,
    fontFamily: TerminalTheme.fontFamily,
    fontSize: 12,
  },
  preview: {
    maxHeight: 120,
    backgroundColor: OkenaColors.surface,
    borderRadius: 6,
    padding: 8,
  },
  plan: {
    maxHeight: 200,
    backgroundColor: OkenaColors.surface,
    borderRadius: 6,
    padding: 8,
  },
  question: { marginBottom: 10 },
  option: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    paddingVertical: 6,
  },
  mark: { color: OkenaColors.textSecondary, fontSize: 16, width: 24 },
  markSelected: { color: OkenaColors.accent },
  input: {
    backgroundColor: OkenaColors.surface,
    borderRadius: 6,
    paddingHorizontal: 10,
    paddingVertical: 6,
    marginTop: 8,
    color: OkenaColors.textPrimary,
    fontSize: 13,
  },
  actions: {
    flexDirection: 'row',
    flexWrap: 'wrap',
    justifyContent: 'flex-end',
    marginTop: 8,
    gap: 8,
  },
  button: {
    paddingHorizontal: 14,
    paddingVertical: 8,
    borderRadius: 8,
    borderWidth: StyleSheet.hairlineWidth,
    borderColor: OkenaColors.borderLight,
  },
  buttonPrimary: {
    backgroundColor: OkenaColors.accent,
    borderColor: OkenaColors.accent,
  },
  buttonDanger: { borderColor: OkenaColors.error },
  buttonDisabled: { opacity: 0.4 },
  buttonText: {
    color: OkenaColors.textPrimary,
    fontSize: 13,
    fontWeight: '600',
  },
  buttonTextPrimary: { color: '#ffffff' },
});
