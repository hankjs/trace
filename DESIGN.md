---
name: Trace
description: AI agent desktop environment for developers
colors:
  surface-deep: "#1f2124"
  surface-base: "#272a2e"
  surface-raised: "#31353a"
  surface-elevated: "#3c4147"
  accent-amber-gold: "#d4a24e"
  accent-amber-gold-hover: "#e0b86a"
  text-primary: "#e8e9eb"
  text-secondary: "#a3a7ad"
  text-muted: "#6e7279"
  border-default: "#3f4349"
  border-subtle: "#33373c"
  error: "#d4634e"
  warning: "#c4a24e"
  success: "#6daa5e"
  info: "#5e8fba"
typography:
  body:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI, system-ui, sans-serif"
    fontSize: "13px"
    fontWeight: 400
    lineHeight: 1.5
    letterSpacing: "0.01em"
  label:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI, system-ui, sans-serif"
    fontSize: "12px"
    fontWeight: 500
    lineHeight: 1.4
  title:
    fontFamily: "-apple-system, BlinkMacSystemFont, Segoe UI, system-ui, sans-serif"
    fontSize: "13px"
    fontWeight: 600
    lineHeight: 1.3
  mono:
    fontFamily: "SF Mono, Cascadia Code, JetBrains Mono, ui-monospace, monospace"
    fontSize: "12px"
    fontWeight: 400
    lineHeight: 1.5
rounded:
  sm: "4px"
  md: "6px"
  lg: "8px"
spacing:
  xs: "4px"
  sm: "8px"
  md: "12px"
  lg: "16px"
  xl: "24px"
  2xl: "32px"
components:
  button-primary:
    backgroundColor: "{colors.accent-amber-gold}"
    textColor: "{colors.surface-deep}"
    rounded: "{rounded.sm}"
    padding: "4px 12px"
  button-primary-hover:
    backgroundColor: "{colors.accent-amber-gold-hover}"
    textColor: "{colors.surface-deep}"
  button-secondary:
    backgroundColor: "transparent"
    textColor: "{colors.text-secondary}"
    rounded: "{rounded.sm}"
    padding: "4px 12px"
  input-default:
    backgroundColor: "{colors.surface-base}"
    textColor: "{colors.text-primary}"
    rounded: "{rounded.md}"
    padding: "8px 12px"
---

# Design System: Trace

## 1. Overview

**Creative North Star: "The Quiet Instrument"**

Trace's visual system is built on the principle that a developer tool should feel like a well-tuned instrument: responsive, predictable, and invisible when things are going well. The interface never competes with the content it displays. Every surface, every transition, every typographic choice serves a single purpose: keeping the developer in flow.

The system rejects chatbot aesthetics (bubbles, avatars, typing indicators), playful SaaS patterns (bouncy animations, gradient accents, rounded everything), and anything that signals "AI product" before "developer tool." The closest physical analogy is a precision measuring instrument: matte black housing, clear markings, no ornamentation.

**Key Characteristics:**
- Controlled density: compact without cramping, breathable without wasting space
- Flat tonal layering: depth through surface lightness, never shadows
- System typography: native fonts at small sizes, hierarchy through weight
- Restrained color: one accent used at less than 10% of any surface
- Keyboard-native: every primary workflow reachable without a mouse

## 2. Colors

A restrained palette of cool blue-slate neutrals with a single muted amber-gold accent. The accent is deliberately non-obvious for a developer tool (avoiding blue, cyan, and purple reflexes).

### Primary
- **Warm Amber-Gold** (oklch(0.74 0.14 70)): Active states, primary actions, current selection indicators. Used on less than 10% of any given screen. Its rarity is the point.

### Neutral
- **Deep Slate** (oklch(0.13 0.008 220)): Root background. The deepest surface.
- **Base Slate** (oklch(0.16 0.008 220)): Navigation panels, sidebars. One step above root.
- **Raised Slate** (oklch(0.20 0.01 220)): Hover states, active items, elevated surfaces.
- **Elevated Slate** (oklch(0.25 0.01 220)): Highest tonal layer. Scrollbar thumbs, pressed states.
- **Primary Text** (oklch(0.92 0.008 220)): Main content, headings, active labels.
- **Secondary Text** (oklch(0.68 0.01 220)): Supporting content, inactive labels.
- **Muted Text** (oklch(0.50 0.008 220)): Timestamps, placeholders, disabled content.
- **Border** (oklch(0.28 0.01 220)): Explicit separators, input outlines.
- **Subtle Border** (oklch(0.22 0.008 220)): Panel dividers, section separators.

### Semantic
- **Error** (oklch(0.65 0.18 25)): Destructive actions, failure states.
- **Warning** (oklch(0.72 0.14 85)): Caution indicators.
- **Success** (oklch(0.65 0.12 155)): Completion, positive states.
- **Info** (oklch(0.65 0.10 240)): Informational badges, in-progress states.

### Named Rules
**The 10% Rule.** The amber-gold accent appears on no more than 10% of any screen. Primary buttons, active nav items, focus rings, and selection indicators only. If you're reaching for the accent color for decoration, stop.

**The No Pure Black Rule.** Every neutral is tinted toward hue 220 at chroma 0.008-0.01. Pure black (#000) and pure white (#fff) are prohibited. The tint is subtle enough to be invisible consciously but prevents the clinical feel of true achromatic surfaces.

## 3. Typography

**Body Font:** System stack (-apple-system, BlinkMacSystemFont, Segoe UI, system-ui, sans-serif)
**Mono Font:** SF Mono, Cascadia Code, JetBrains Mono, ui-monospace, monospace

**Character:** Native, invisible, fast. The system font stack gives Trace a platform-native feel on every OS. No custom fonts to load, no FOUT, no personality competing with the developer's code.

### Hierarchy
- **Title** (600, 13px, 1.3): Section headers, view titles, nav section labels. Same size as body; weight carries the hierarchy.
- **Body** (400, 13px, 1.5, letter-spacing 0.01em): All content text, messages, descriptions. Slightly boosted letter-spacing for dark-on-light readability.
- **Label** (500, 12px, 1.4): Button text, table headers, metadata, timestamps.
- **Caption** (400, 11px, 1.4): Badges, status indicators, secondary metadata.
- **Mono** (400, 12px, 1.5): Code blocks, file paths, terminal output, technical values.

### Named Rules
**The Tight Scale Rule.** The entire type system spans only 11px to 13px. Hierarchy is achieved through weight (400/500/600) and color (primary/secondary/muted), never through dramatic size differences. This is a dense tool, not a marketing page.

**The Dark Compensation Rule.** Light text on dark backgrounds gets +0.01em letter-spacing and line-height 1.5 minimum. Both are already baked into the body defaults.

## 4. Elevation

Trace uses flat tonal layering exclusively. No box-shadows anywhere in the system. Depth is communicated through surface lightness: darker surfaces recede, lighter surfaces advance.

### Tonal Layers
- **Layer 0** (oklch 0.13): Root canvas. The void.
- **Layer 1** (oklch 0.16): Panels (nav, sidebars). Sits on top of root.
- **Layer 2** (oklch 0.20): Interactive surfaces (hover states, active items, dropdowns).
- **Layer 3** (oklch 0.25): Highest layer (pressed states, scrollbar thumbs).

### Named Rules
**The No Shadow Rule.** Shadows are prohibited. If a surface needs to feel elevated, it gets a lighter tonal value. If a dropdown needs separation from its parent, it gets a border and a tonal step. Shadows imply physicality; Trace is a flat instrument.

## 5. Components

### Buttons
- **Shape:** Slightly rounded (4px radius). Not pill-shaped, not sharp.
- **Primary:** Green-gold background, deep-slate text. Padding 4px 12px. Font-size 11-12px, weight 500.
- **Hover:** Lighter amber-gold (oklch 0.80). No transform, no shadow.
- **Secondary/Ghost:** Transparent background, 1px border in border color, secondary text. Hover fills with surface-hover.
- **Disabled:** 40% opacity, cursor not-allowed.

### Inputs
- **Style:** 1px border (border-subtle at rest, accent on focus), surface-1 background, radius-md (6px).
- **Focus:** Border shifts to accent color. No glow, no ring, no shadow.
- **Sizing:** Padding 8px 12px, font-size 12-13px.
- **Placeholder:** Muted text color.

### Navigation (Left Panel)
- **Width:** 220px expanded, 48px collapsed (icon-only).
- **Section headers:** 12px, weight 500, secondary text color. Click to navigate.
- **Active item:** Accent text color. No background change on the header itself.
- **Session items:** 12px, secondary text, left-indented. Active gets surface-2 background.
- **Collapse:** Cmd+B toggle. Transition 200ms ease-out-expo.

### Tables
- **Headers:** 11px, weight 500, muted text, uppercase not used.
- **Rows:** 12px body text. Bottom border (border-subtle). Hover fills surface-hover.
- **Density:** Padding 8px per cell. No extra vertical spacing between rows.

### Status Badges
- **Shape:** 3px radius, padding 1px 6px, font-size 10px, weight 500.
- **Colors:** Semantic surface + semantic text (e.g. info-surface background, info text color).
- **No borders on badges.** The tinted background is sufficient.

### View Headers
- **Height:** 36px fixed. Thin contextual bar at top of center content.
- **Content:** View title (12px, weight 500, secondary text) left-aligned. Actions right-aligned.
- **Border:** 1px bottom border-subtle.

## 6. Do's and Don'ts

### Do:
- **Do** use tonal layering (surface-0 through surface-3) to create depth. Darker recedes, lighter advances.
- **Do** keep the accent under 10% of any screen. Count the amber-gold pixels; if it feels like more than a few touches, remove some.
- **Do** use the system font stack. It loads instantly and feels native on every platform.
- **Do** maintain 13px as the base size. Hierarchy through weight and color, not size.
- **Do** use tables for list data. They're denser and more scannable than cards.
- **Do** use inline patterns (dropdowns, expanding sections) instead of modals wherever possible.
- **Do** respect prefers-reduced-motion. All transitions collapse to near-zero.
- **Do** use Cmd+key shortcuts for panel toggles and primary actions.

### Don't:
- **Don't** use box-shadows anywhere. Not on dropdowns, not on modals, not on hover. Flat tonal layering only.
- **Don't** use chat bubbles, avatars, or typing indicators. Trace is a development environment, not a chatbot.
- **Don't** use bouncy or elastic easing. Ease-out-expo only (cubic-bezier 0.16, 1, 0.3, 1).
- **Don't** use border-left or border-right greater than 1px as colored accents on list items or cards.
- **Don't** use gradient text (background-clip: text).
- **Don't** use cards for list data. Tables or plain rows with borders are denser and more appropriate.
- **Don't** use modals as the first solution. Inline expansion, dropdowns, and progressive disclosure first.
- **Don't** use font sizes larger than 14px anywhere in the product UI. This is a dense tool.
- **Don't** use rounded corners larger than 8px. Nothing should look pill-shaped or bubbly.
- **Don't** use decorative motion. Transitions serve state feedback only (120-350ms range).
