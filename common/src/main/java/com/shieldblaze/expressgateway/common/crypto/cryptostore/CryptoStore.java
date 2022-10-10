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
package com.shieldblaze.expressgateway.common.crypto.cryptostore;

import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;

import javax.crypto.spec.PBEParameterSpec;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
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
import java.util.Iterator;

/**
 * {@link CryptoStore} is implementation of {@link KeyStore} which
 * simplifies store and fetch operations of {@link Certificate} and
 * {@link PrivateKey}.
 */
public final class CryptoStore {
    private static final Logger logger = LogManager.getLogger(CryptoStore.class);

    /**
     * Store an entry of {@link PrivateKey} and {@link X509Certificate}s in {@link KeyStore}
     *
     * @param inputStream  {@link InputStream} containing {@link KeyStore} data if entry has to
     *                     be stored in existing {@link KeyStore}. Else, this
     *                     can be set to {@code null} if we want to create new
     *                     {@link KeyStore}
     * @param outputStream {@link OutputStream} where {@link KeyStore} data will be stored
     * @param password     Password to secure the entry
     * @param alias        Alias name of entry
     * @param privateKey   {@link PrivateKey} to store with this entry
     * @param certificates {@link Certificate} associated with this entry
     * @throws KeyStoreException        Thrown in case of exception with {@link KeyStore}
     * @throws CertificateException     Thrown in case of exception with {@link Certificate}
     * @throws IOException              Thrown in case of exception with I/O streams
     * @throws NoSuchAlgorithmException Thrown in case of unavailability of PKCS12 in {@link KeyStore}
     */
    public static void storePrivateKeyAndCertificate(InputStream inputStream, OutputStream outputStream, char[] password, String alias,
                                                     PrivateKey privateKey, Certificate... certificates)
            throws KeyStoreException, CertificateException, IOException, NoSuchAlgorithmException {

        KeyStore keyStore = KeyStore.getInstance("PKCS12");
        keyStore.load(inputStream, password);

        byte[] salt = new byte[20];
        secureRandom().nextBytes(salt);

        // Delete any previous key entry
        Iterator<String> iterator = keyStore.aliases().asIterator();
        while (iterator.hasNext()) {
            String str = iterator.next();
            if (str.equalsIgnoreCase(alias)) {
                keyStore.deleteEntry(alias);
            }
        }

        keyStore.setEntry(alias, new KeyStore.PrivateKeyEntry(privateKey, certificates),
                new KeyStore.PasswordProtection(password, "PBEWithHmacSHA512AndAES_256", new PBEParameterSpec(salt, 1024)));

        keyStore.store(outputStream, password);
        outputStream.flush();
    }

    /**
     * Fetch entry of {@link PrivateKey} and {@link X509Certificate}s from {@link KeyStore}
     *
     * @param inputStream {@link InputStream} containing {@link KeyStore} data
     * @param password    Password to secure the entry
     * @param alias       Alias name of entry
     * @return {@link CryptoEntry} instance holding {@link PrivateKey} and {@link X509Certificate}s
     * @throws UnrecoverableKeyException Thrown when entry cannot be recovered
     * @throws IOException               Thrown in case of exception with I/O streams
     * @throws CertificateException      Thrown in case of exception with {@link X509Certificate}
     * @throws KeyStoreException         Thrown in case of exception with {@link KeyStore}
     * @throws NoSuchAlgorithmException  Thrown in case of unavailability of PKCS12 in {@link KeyStore}
     */
    public static CryptoEntry fetchPrivateKeyCertificateEntry(InputStream inputStream, char[] password, String alias) throws UnrecoverableKeyException, IOException,
            CertificateException, KeyStoreException, NoSuchAlgorithmException {
        KeyStore keyStore = KeyStore.getInstance("PKCS12");
        keyStore.load(inputStream, password);

        Certificate[] certificates = keyStore.getCertificateChain(alias);
        Key privateKey = keyStore.getKey(alias, password);

        return new CryptoEntry((PrivateKey) privateKey, certificates);
    }

    private static SecureRandom secureRandom() {
        try {
            return SecureRandom.getInstanceStrong();
        } catch (NoSuchAlgorithmException e) {
            logger.error("SecureRandom Strongest Algorithm not available");
            return new SecureRandom();
        }
    }

    private CryptoStore() {
        // Prevent outside initialization
    }
}
