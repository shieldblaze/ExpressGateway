/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.common.datastore;

import com.shieldblaze.expressgateway.common.utils.SystemPropertyUtil;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import javax.crypto.spec.PBEParameterSpec;
import java.io.File;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.nio.file.Files;
import java.security.Key;
import java.security.KeyStore;
import java.security.KeyStoreException;
import java.security.NoSuchAlgorithmException;
import java.security.PrivateKey;
import java.security.SecureRandom;
import java.security.UnrecoverableKeyException;
import java.security.cert.Certificate;
import java.security.cert.CertificateException;
import java.security.cert.X509Certificate;
import java.util.Arrays;
import java.util.Iterator;

/**
 * {@link DataStore} is implementation of {@link KeyStore} which
 * simplifies store and fetch operations of {@link Certificate} and
 * {@link PrivateKey}.
 */
public final class DataStore {
    private static final Logger logger = LogManager.getLogger(DataStore.class);
    public static final DataStore INSTANCE = new DataStore();
    private static final File FILENAME = new File(SystemPropertyUtil.getPropertyOrEnv("datastore.file", "datastore.p12"));

    private DataStore() {
        // Prevent outside initialization
    }

    /**
     * Store an entry of {@link PrivateKey} and {@link X509Certificate}s
     *
     * @param password         Password to use for storing
     * @param alias            Alias name
     * @param privateKey       {@link PrivateKey} instance
     * @param x509Certificates {@link X509Certificate} chains
     */
    public void store(char[] password, String alias, PrivateKey privateKey, X509Certificate... x509Certificates) {
        password = password.clone();
        FileInputStream fis = null;
        try {
            SecureRandom secureRandom;
            try {
                secureRandom = SecureRandom.getInstanceStrong();
            } catch (NoSuchAlgorithmException e) {
                logger.error("SecureRandom Strongest Algorithm not available");
                secureRandom = new SecureRandom();
            }

            if (FILENAME.exists()) {
                fis = new FileInputStream(FILENAME);
            }

            KeyStore keyStore = KeyStore.getInstance("PKCS12");
            keyStore.load(fis, password);

            byte[] salt = new byte[20];
            secureRandom.nextBytes(salt);

            // Delete any previous key entry
            Iterator<String> iterator = keyStore.aliases().asIterator();
            while (iterator.hasNext()) {
                String str = iterator.next();
                if (str.equalsIgnoreCase(alias)) {
                    keyStore.deleteEntry(alias);
                }
            }

            keyStore.setEntry(alias, new KeyStore.PrivateKeyEntry(privateKey, x509Certificates),
                    new KeyStore.PasswordProtection(password, "PBEWithHmacSHA512AndAES_256",
                            new PBEParameterSpec(salt, 1_000_000)));

            FileOutputStream fos = new FileOutputStream(FILENAME, false);
            keyStore.store(fos, password);
            fos.close();
        } catch (KeyStoreException e) {
            logger.fatal("KeyStore PKCS12 is not available. This should never happen!", e);
            System.exit(1);
        } catch (IOException e) {
            e.printStackTrace();
            logger.fatal("Failed loading KeyStore");
        } catch (CertificateException | NoSuchAlgorithmException e) {
            logger.error("Failed to store operate into KeyStore", e);
        } finally {
            // Clear the password array
            Arrays.fill(password, '0');
        }
    }

    /**
     * Get entire DataStore file as byte-array
     *
     * @return byte-array of file
     * @throws IOException Incase of error during reading DataStore file
     */
    public byte[] getEntire() throws IOException {
        try {
            return Files.readAllBytes(FILENAME.toPath());
        } catch (IOException e) {
            logger.error("Failed to read DataStore file", e);
            throw e;
        }
    }

    public Entry get(char[] password, String alias)
            throws UnrecoverableKeyException, IOException, CertificateException, KeyStoreException, NoSuchAlgorithmException {
        password = password.clone();
        try (FileInputStream fis = new FileInputStream(FILENAME)) {
            KeyStore keyStore = KeyStore.getInstance("PKCS12");
            keyStore.load(FILENAME.exists() ? fis : null, password);

            Certificate[] certificates = keyStore.getCertificateChain(alias);
            Key privateKey = keyStore.getKey(alias, password);

            return new Entry((PrivateKey) privateKey, certificates);
        } catch (IOException e) {
            logger.fatal("Failed loading KeyStore");
            throw e;
        } catch (CertificateException | KeyStoreException | NoSuchAlgorithmException e) {
            logger.error("Failed to operate entries into KeyStore", e);
            throw e;
        } catch (UnrecoverableKeyException e) {
            logger.error("Failed to store operate into KeyStore", e);
            throw e;
        } finally {
            // Clear the password array
            Arrays.fill(password, '0');
        }
    }

    /**
     * Destroy the Datastore file
     */
    public boolean destroy() {
        if (FILENAME.exists()) {
            boolean deleted = FILENAME.delete();
            if (!deleted) {
                logger.error("Failed to delete DataStore file");
            }
            return deleted;
        } else {
            logger.error("DataStore file does not exists");
        }
        return false;
    }
}
