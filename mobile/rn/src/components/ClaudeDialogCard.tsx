/**
 * ClaudeDialogCard.tsx — one open Claude dialog with the controls to answer
 * it. The card kind follows `tool_name`: `AskUserQuestion` → questions,
 * `ExitPlanMode` → plan approval, anything else → allow / deny. Denying (and
 * sending plan feedback) is a second step with an optional message, so the
 * common answer is one tap.
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
import { ChatMarkdown } from './ChatMarkdown';

export interface ClaudeDialogCardProps {
  dialog: ApiClaudeDialog;
  onDecide: (decision: ApiClaudeDecision) => void;
  /** The message step (deny / keep planning) was laid out; show its buttons. */
  onRevealStep: () => void;
}

export const ClaudeDialogCard: React.FC<ClaudeDialogCardProps> = ({
  dialog,
  onDecide,
  onRevealStep,
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
        onRevealStep={onRevealStep}
      />
    );
  }
  return (
    <PermissionCard
      dialog={dialog}
      onDecide={onDecide}
      onRevealStep={onRevealStep}
    />
  );
};

// ── default tool: allow / always allow / deny ────────────────────────────────

const PermissionCard: React.FC<ClaudeDialogCardProps> = ({
  dialog,
  onDecide,
  onRevealStep,
}) => {
  const [denying, setDenying] = useState(false);
  return (
    <Card kind="Permission" title={dialog.tool_name}>
      <ScrollView style={styles.preview} nestedScrollEnabled>
        <Text style={styles.mono} selectable>
          {previewClaudeToolInput(dialog.tool_input)}
        </Text>
      </ScrollView>
      {denying ? (
        <MessageStep
          placeholder="Tell Claude why (optional)"
          confirmLabel="Deny"
          danger
          onCancel={() => setDenying(false)}
          onConfirm={(message) => onDecide(denial(message))}
          onReveal={onRevealStep}
        />
      ) : (
        <View style={styles.actions}>
          <CardButton label="Deny" danger onPress={() => setDenying(true)} />
          {dialog.permission_suggestions.length > 0 ? (
            <CardButton
              label="Always"
              onPress={() => onDecide({ decision: 'allow', always: true })}
            />
          ) : null}
          <CardButton
            label="Allow"
            primary
            onPress={() => onDecide({ decision: 'allow' })}
          />
        </View>
      )}
    </Card>
  );
};

// ── ExitPlanMode: approve / keep planning ────────────────────────────────────

const PlanCard: React.FC<{
  // Without a plan file Claude sends no plan; it is then in the transcript.
  plan: string | undefined;
  onDecide: ClaudeDialogCardProps['onDecide'];
  onRevealStep: ClaudeDialogCardProps['onRevealStep'];
}> = ({ plan, onDecide, onRevealStep }) => {
  const [revising, setRevising] = useState(false);
  return (
    <Card kind="Plan" title="Ready to start?">
      {plan !== undefined ? (
        <ScrollView style={styles.plan} nestedScrollEnabled>
          <ChatMarkdown text={plan} />
        </ScrollView>
      ) : (
        <Text style={styles.muted}>The plan is in the conversation above.</Text>
      )}
      {revising ? (
        <MessageStep
          placeholder="What should change? (optional)"
          confirmLabel="Keep planning"
          onCancel={() => setRevising(false)}
          onConfirm={(message) => onDecide(denial(message))}
          onReveal={onRevealStep}
        />
      ) : (
        <View style={styles.actions}>
          <CardButton label="Keep planning" onPress={() => setRevising(true)} />
          <CardButton
            label="Approve"
            primary
            onPress={() => onDecide({ decision: 'allow' })}
          />
        </View>
      )}
    </Card>
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
    <Card
      kind={questions.length > 1 ? `${questions.length} questions` : 'Question'}
    >
      {questions.map((question, questionIndex) => {
        const selection = selections[question.question] ?? EMPTY_SELECTION;
        return (
          <View key={questionIndex} style={styles.question}>
            {question.header ? (
              <Text style={styles.header}>{question.header}</Text>
            ) : null}
            <Text style={styles.questionText}>{question.question}</Text>
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
                keyboardAppearance="dark"
                autoFocus
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
    </Card>
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
    style={({ pressed }) => [
      styles.option,
      selected && styles.optionSelected,
      pressed && styles.pressed,
    ]}
    onPress={onPress}
    accessibilityRole={multiSelect ? 'checkbox' : 'radio'}
    accessibilityState={{ checked: selected }}
  >
    <View
      style={[
        styles.mark,
        multiSelect ? styles.markSquare : styles.markRound,
        selected && styles.markSelected,
      ]}
    >
      {selected ? (
        <View
          style={[
            styles.markFill,
            multiSelect ? styles.markFillSquare : styles.markFillRound,
          ]}
        />
      ) : null}
    </View>
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

const Card: React.FC<{
  kind: string;
  title?: string;
  children: React.ReactNode;
}> = ({ kind, title, children }) => (
  <View style={styles.card}>
    <Text style={styles.kind}>{kind.toUpperCase()}</Text>
    {title !== undefined ? <Text style={styles.title}>{title}</Text> : null}
    {children}
  </View>
);

/** The second step of deny / keep planning: an optional message for Claude. */
const MessageStep: React.FC<{
  placeholder: string;
  confirmLabel: string;
  danger?: boolean;
  onCancel: () => void;
  onConfirm: (message: string) => void;
  onReveal: () => void;
}> = ({ placeholder, confirmLabel, danger, onCancel, onConfirm, onReveal }) => {
  const [message, setMessage] = useState('');
  return (
    <View onLayout={onReveal}>
      <TextInput
        style={styles.input}
        value={message}
        onChangeText={setMessage}
        placeholder={placeholder}
        placeholderTextColor={OkenaColors.textTertiary}
        keyboardAppearance="dark"
        multiline
      />
      <View style={styles.actions}>
        <CardButton label="Cancel" onPress={onCancel} />
        <CardButton
          label={confirmLabel}
          primary={!danger}
          danger={danger}
          onPress={() => onConfirm(message)}
        />
      </View>
    </View>
  );
};

const CardButton: React.FC<{
  label: string;
  onPress: () => void;
  primary?: boolean;
  danger?: boolean;
  disabled?: boolean;
}> = ({ label, onPress, primary, danger, disabled }) => (
  <Pressable
    style={({ pressed }) => [
      styles.button,
      primary && styles.buttonPrimary,
      danger && styles.buttonDanger,
      disabled && styles.buttonDisabled,
      pressed && styles.pressed,
    ]}
    disabled={disabled}
    onPress={onPress}
    accessibilityRole="button"
  >
    <Text
      style={[
        styles.buttonText,
        primary && styles.buttonTextPrimary,
        danger && styles.buttonTextDanger,
      ]}
      numberOfLines={1}
    >
      {label}
    </Text>
  </Pressable>
);

const styles = StyleSheet.create({
  flex: { flex: 1 },
  card: {
    backgroundColor: OkenaColors.surfaceElevated,
    borderRadius: 14,
    borderWidth: 1,
    borderColor: OkenaColors.warningBorder,
    padding: 14,
    marginBottom: 8,
  },
  kind: {
    color: OkenaColors.warning,
    fontSize: 11,
    fontWeight: '700',
    letterSpacing: 0.6,
    marginBottom: 4,
  },
  title: {
    color: OkenaColors.textPrimary,
    fontSize: 16,
    fontWeight: '600',
    marginBottom: 10,
  },
  header: {
    color: OkenaColors.accent,
    fontSize: 12,
    fontWeight: '600',
    marginBottom: 2,
  },
  questionText: {
    color: OkenaColors.textPrimary,
    fontSize: 15,
    fontWeight: '600',
    lineHeight: 21,
    marginBottom: 8,
  },
  body: { color: OkenaColors.textPrimary, fontSize: 15, lineHeight: 21 },
  muted: { color: OkenaColors.textSecondary, fontSize: 14, lineHeight: 20 },
  description: {
    color: OkenaColors.textSecondary,
    fontSize: 13,
    lineHeight: 18,
    marginTop: 2,
  },
  mono: {
    color: OkenaColors.textPrimary,
    fontFamily: TerminalTheme.fontFamily,
    fontSize: 12,
    lineHeight: 17,
  },
  preview: {
    maxHeight: 160,
    flexGrow: 0,
    backgroundColor: OkenaColors.background,
    borderRadius: 8,
    padding: 10,
  },
  plan: {
    maxHeight: 280,
    flexGrow: 0,
    backgroundColor: OkenaColors.background,
    borderRadius: 8,
    padding: 10,
  },
  question: { marginBottom: 12 },
  option: {
    flexDirection: 'row',
    alignItems: 'flex-start',
    minHeight: 44,
    paddingVertical: 10,
    paddingHorizontal: 10,
    marginBottom: 6,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: OkenaColors.borderLight,
  },
  optionSelected: {
    borderColor: OkenaColors.accent,
    backgroundColor: OkenaColors.accentSoft,
  },
  mark: {
    width: 20,
    height: 20,
    marginTop: 1,
    marginRight: 12,
    borderWidth: 2,
    borderColor: OkenaColors.textTertiary,
    alignItems: 'center',
    justifyContent: 'center',
  },
  markRound: { borderRadius: 10 },
  markSquare: { borderRadius: 5 },
  markSelected: { borderColor: OkenaColors.accent },
  markFill: { backgroundColor: OkenaColors.accent },
  markFillRound: { width: 10, height: 10, borderRadius: 5 },
  markFillSquare: { width: 10, height: 10, borderRadius: 2 },
  input: {
    backgroundColor: OkenaColors.background,
    borderRadius: 10,
    borderWidth: 1,
    borderColor: OkenaColors.borderLight,
    paddingHorizontal: 12,
    paddingVertical: 10,
    marginTop: 10,
    maxHeight: 120,
    color: OkenaColors.textPrimary,
    fontSize: 15,
  },
  actions: {
    flexDirection: 'row',
    marginTop: 12,
    gap: 8,
  },
  button: {
    flex: 1,
    height: 44,
    borderRadius: 10,
    alignItems: 'center',
    justifyContent: 'center',
    paddingHorizontal: 8,
    backgroundColor: OkenaColors.surfaceOverlay,
    borderWidth: 1,
    borderColor: OkenaColors.borderLight,
  },
  buttonPrimary: {
    backgroundColor: OkenaColors.accent,
    borderColor: OkenaColors.accent,
  },
  buttonDanger: { borderColor: OkenaColors.errorBorder },
  buttonDisabled: { opacity: 0.4 },
  pressed: { opacity: 0.7 },
  buttonText: {
    color: OkenaColors.textPrimary,
    fontSize: 15,
    fontWeight: '600',
  },
  buttonTextPrimary: { color: '#ffffff' },
  buttonTextDanger: { color: OkenaColors.error },
});
