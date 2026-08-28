# INDEX.md — AGENT ENGINEERING FRAMEWORK USAGE GUIDE

(C) Qunix Project — Mohammad Muzamil 2026  
All rights reserved.

---

# 1. PURPOSE OF THIS DIRECTORY

This directory contains **AI execution specification files** designed to enhance autonomous coding agents during:

- Operating System development (kernel, drivers, bootloader)
- Debugging complex systems
- Adding new features to large-scale codebases
- Architecture redesign
- Performance and stability engineering

These files are NOT documentation.

They are **behavioral execution engines for AI coding systems**.

---

# 2. WHEN TO USE THESE FILES

Use these specifications when:

## 2.1 Debugging Complex Systems
- Kernel panic / crash issues
- Memory corruption / leaks
- Bootloader or hardware init failures
- Race conditions or concurrency bugs

## 2.2 Feature Development Without Clear Implementation Path
- You know WHAT to build but not HOW
- System design exists but implementation is unclear
- You need step-by-step execution planning

## 2.3 OS / Low-Level Development
- Scheduling systems
- Memory allocators
- File systems
- Device drivers
- System call interfaces

## 2.4 Large Architecture Changes
- Refactoring core kernel architecture
- Migrating subsystems
- Introducing new execution layers

---

# 3. FILE USAGE MAP (AGENT ROUTING)

Each file defines a different AI “engine mode”:

## 3.1 CLAUDE.md
Use when:
- You want strict correctness-first engineering
- You are building or debugging OS-level components
- You need conservative, safe execution

Prompt example:
> "Use ./agents/CLAUDE.md rules. Debug kernel panic in scheduler module."

---

## 3.2 CODEX.md
Use when:
- You want fast production-grade implementation
- You need structured code generation with verification
- You are implementing systems from spec to code

Prompt example:
> "Follow ./agents/CODEX.md. Implement virtual memory manager with paging and swap support."

---

## 3.3 OTHERS.md
Use when:
- You need multi-agent reasoning (architecture + testing + building)
- Complex systems requiring validation and consensus
- Large distributed or OS-scale systems

Prompt example:
> "Use ./agents/OTHERS.md. Design and implement full OS scheduler with multi-core support."

---

# 4. HOW TO PROMPT THE AGENT

To activate these systems, explicitly reference the file:

## Example 1 — Debugging OS Kernel

> "Use ./agents/CLAUDE.md rules. I have a kernel panic during boot after initializing memory manager. Diagnose and fix."

## Example 2 — New Feature Implementation

> "Use ./agents/CODEX.md. Add virtual filesystem layer supporting ext4 and tmpfs in my OS kernel."

## Example 3 — Full System Design

> "Use ./agents/OTHERS.md. Design and implement a full microkernel architecture with process isolation, IPC, and scheduling."

---

# 5. IMPORTANT BEHAVIORAL RULE

These files override default AI behavior.

When activated:
- The agent must follow the specified execution model strictly
- No assumptions allowed without declaration
- All outputs must be verifiable
- All designs must be production-grade

---

# 6. INTEGRATION NOTE (OPTIONAL INSTALLATION)

If you want to enable these frameworks in your own project:

1. Place files in project root or `/agents/` directory:
   - CLAUDE.md
   - CODEX.md
   - OTHERS.md
   - index.md

2. Reference them explicitly in prompts to your AI coding assistant.

3. Ensure your development workflow includes verification-based iteration.

---

# 7. FINAL NOTICE

These frameworks are designed for:
- advanced systems programming
- OS development
- compiler/runtime engineering
- distributed systems design

They are not required for simple scripting tasks.

---

# COPYRIGHT NOTICE

(C) Qunix Project — Mohammad Muzamil 2026  
Unauthorized redistribution of these files in ./agents/ without attribution is prohibited.
