#!/usr/bin/env node
import { run } from './launcher.mjs';

process.exitCode = run('lpcli');
