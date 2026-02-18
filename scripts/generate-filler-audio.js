#!/usr/bin/env node
/**
 * Generate pre-cached filler phrase audio files
 * Usage: OPENAI_API_KEY=sk-... node scripts/generate-filler-audio.js
 */

import fs from 'fs';
import path from 'path';
import https from 'https';
import { fileURLToPath } from 'url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const OPENAI_API_KEY = process.env.OPENAI_API_KEY;
if (!OPENAI_API_KEY) {
  console.error('Error: OPENAI_API_KEY environment variable is required');
  process.exit(1);
}

const OUTPUT_DIR = path.join(__dirname, '../web/audio/fillers');
if (!fs.existsSync(OUTPUT_DIR)) {
  fs.mkdirSync(OUTPUT_DIR, { recursive: true });
}

// Filler phrases with metadata
const FILLERS = {
  ja: [
    { text: 'ええっと、それはですね', file: 'ja-01-eetto.mp3', voice: 'alloy' },
    { text: 'うーん、ちょっと考えますね', file: 'ja-02-uun.mp3', voice: 'alloy' },
    { text: 'なるほど、それでは', file: 'ja-03-naruhodo.mp3', voice: 'alloy' },
    { text: 'そうですね、まず', file: 'ja-04-soudesune.mp3', voice: 'alloy' },
    { text: 'いい質問ですね', file: 'ja-05-iishitsumon.mp3', voice: 'alloy' },
    { text: 'ちょっと待ってくださいね', file: 'ja-06-chotto.mp3', voice: 'alloy' },
    { text: 'えーっと、どうだったかな', file: 'ja-07-eetto2.mp3', voice: 'alloy' },
  ],
  en: [
    { text: 'Let me think about that', file: 'en-01-think.mp3', voice: 'nova' },
    { text: "Hmm, that's interesting", file: 'en-02-hmm.mp3', voice: 'nova' },
    { text: "Well, let's see", file: 'en-03-well.mp3', voice: 'nova' },
    { text: 'Good question', file: 'en-04-good.mp3', voice: 'nova' },
    { text: 'One moment please', file: 'en-05-moment.mp3', voice: 'nova' },
  ]
};

async function generateTTS(text, voice, outputPath) {
  return new Promise((resolve, reject) => {
    const postData = JSON.stringify({
      model: 'tts-1',
      voice: voice,
      input: text,
      speed: 0.9, // Slightly slower for natural feel
    });

    const options = {
      hostname: 'api.openai.com',
      port: 443,
      path: '/v1/audio/speech',
      method: 'POST',
      headers: {
        'Content-Type': 'application/json',
        'Authorization': `Bearer ${OPENAI_API_KEY}`,
        'Content-Length': Buffer.byteLength(postData),
      },
    };

    console.log(`Generating: "${text}" → ${path.basename(outputPath)}`);

    const req = https.request(options, (res) => {
      if (res.statusCode !== 200) {
        reject(new Error(`TTS API returned ${res.statusCode}`));
        return;
      }

      const fileStream = fs.createWriteStream(outputPath);
      res.pipe(fileStream);

      fileStream.on('finish', () => {
        fileStream.close();
        const stats = fs.statSync(outputPath);
        console.log(`  ✓ Saved ${(stats.size / 1024).toFixed(1)}KB`);
        resolve();
      });

      fileStream.on('error', reject);
    });

    req.on('error', reject);
    req.write(postData);
    req.end();
  });
}

async function main() {
  console.log('🎙️  Generating filler phrase audio files...\n');

  for (const [lang, phrases] of Object.entries(FILLERS)) {
    console.log(`\n📝 ${lang.toUpperCase()} phrases:`);
    for (const phrase of phrases) {
      const outputPath = path.join(OUTPUT_DIR, phrase.file);
      try {
        await generateTTS(phrase.text, phrase.voice, outputPath);
        // Rate limit: 3 requests per minute (tier 1) = 20 seconds per request
        await new Promise(r => setTimeout(r, 21000));
      } catch (err) {
        console.error(`  ✗ Failed: ${err.message}`);
      }
    }
  }

  // Generate manifest.json
  const manifest = {
    version: 1,
    generated: new Date().toISOString(),
    fillers: FILLERS,
  };
  const manifestPath = path.join(OUTPUT_DIR, 'manifest.json');
  fs.writeFileSync(manifestPath, JSON.stringify(manifest, null, 2));
  console.log(`\n✅ Generated manifest: ${manifestPath}`);
  console.log('\n🎉 All done! Audio files ready in web/audio/fillers/');
}

main().catch(err => {
  console.error('Fatal error:', err);
  process.exit(1);
});
