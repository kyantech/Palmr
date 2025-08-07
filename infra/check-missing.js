const { PrismaClient } = require('@prisma/client');
const fs = require('fs');
const path = require('path');

const prisma = new PrismaClient();

const loadConfigs = () => {
  try {
    const configsPath = path.join(__dirname, 'configs.json');
    const configs = JSON.parse(fs.readFileSync(configsPath, 'utf8'));
    
    return Object.values(configs).flat();
  } catch (error) {
    console.error('Error loading configs:', error.message);
    return [];
  }
};

const loadProviders = () => {
  try {
    const providersPath = path.join(__dirname, 'providers.json');
    return JSON.parse(fs.readFileSync(providersPath, 'utf8'));
  } catch (error) {
    console.error('Error loading providers:', error.message);
    return [];
  }
};

async function checkSeedingNeeded() {
  try {
    const appConfigCount = await prisma.appConfig.count();
    const userCount = await prisma.user.count();
    const authProviderCount = await prisma.authProvider.count();
    
    if (appConfigCount === 0 || userCount === 0) {
      console.log('true');
      return;
    }
    
    if (authProviderCount === 0) {
      console.log('true');
      return;
    }
    
    const allConfigs = loadConfigs();
    const existingConfigs = await prisma.appConfig.findMany({
      where: {
        key: {
          in: allConfigs
        }
      },
      select: { key: true }
    });
    const existingConfigKeys = existingConfigs.map(c => c.key);
    const missingConfigs = allConfigs.filter(key => !existingConfigKeys.includes(key));
    
    if (missingConfigs.length > 0) {
      console.log('true');
      return;
    }
    
    const expectedProviders = loadProviders();
    const existingProviders = await prisma.authProvider.findMany({
      select: { name: true }
    });
    const existingProviderNames = existingProviders.map(p => p.name);
    const missingProviders = expectedProviders.filter(name => !existingProviderNames.includes(name));
    
    if (missingProviders.length > 0) {
      console.log('true');
      return;
    }
    
    console.log('false');
  } catch (error) {
    console.error('Error checking if seeding is needed:', error);
    console.log('true');
  } finally {
    await prisma.$disconnect();
  }
}

async function checkMissingProviders() {
  try {
    const expectedProviders = loadProviders();
    const existingProviders = await prisma.authProvider.findMany({
      select: { name: true }
    });
    const existingProviderNames = existingProviders.map(p => p.name);
    const missingProviders = expectedProviders.filter(name => !existingProviderNames.includes(name));
    
    if (missingProviders.length > 0) {
      console.log('Missing providers: ' + missingProviders.join(', '));
    } else {
      console.log('No missing providers');
    }
  } catch (error) {
    console.error('Error checking missing providers:', error);
    console.log('Error checking providers');
  } finally {
    await prisma.$disconnect();
  }
}

async function checkMissingConfigs() {
  try {
    const allConfigs = loadConfigs();
    const existingConfigs = await prisma.appConfig.findMany({
      where: {
        key: {
          in: allConfigs
        }
      },
      select: { key: true }
    });
    const existingConfigKeys = existingConfigs.map(c => c.key);
    const missingConfigs = allConfigs.filter(key => !existingConfigKeys.includes(key));
    
    if (missingConfigs.length > 0) {
      console.log('Missing configurations: ' + missingConfigs.join(', '));
    } else {
      console.log('No missing configurations');
    }
  } catch (error) {
    console.error('Error checking missing configurations:', error);
    console.log('Error checking configurations');
  } finally {
    await prisma.$disconnect();
  }
}

const command = process.argv[2];

switch (command) {
  case 'check-seeding':
    checkSeedingNeeded();
    break;
  case 'check-providers':
    checkMissingProviders();
    break;
  case 'check-configs':
    checkMissingConfigs();
    break;
  default:
    console.error('Unknown command. Use: check-seeding, check-providers, or check-configs');
    process.exit(1);
}