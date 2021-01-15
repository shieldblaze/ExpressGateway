/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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
package com.shieldblaze.expressgateway.configuration.tls;

import com.shieldblaze.expressgateway.common.utils.Profile;
import org.bouncycastle.jce.provider.BouncyCastleProvider;

import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.security.KeyStore;
import java.security.PrivateKey;
import java.security.cert.Certificate;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.Iterator;
import java.util.List;
import java.util.Map;

/**
 * {@linkplain KeyStoreHandler} handles {@linkplain TLSConfiguration} for both server and client.
 * Certificates and Private Keys are stored in password-protected PKCS12 vault.
 */
final class KeyStoreHandler {

    /**
     * Save server certificates and private keys into PKCS12 password-protected store.
     *
     * @param tlsConfiguration {@link TLSConfiguration} containing certificates and private keys.
     * @param profile          Profile of this configuration.
     * @param password         Password to use for encryption PKCS12 store.
     * @throws Exception If any error occurs
     */
    static void saveServer(TLSConfiguration tlsConfiguration, String profile, String password) throws Exception {
        KeyStore keyStore = KeyStore.getInstance("PKCS12", new BouncyCastleProvider());
        keyStore.load(null, null);

        for (Map.Entry<String, CertificateKeyPair> entry : tlsConfiguration.certificateKeyPairMap().entrySet()) {

            if (entry.getValue().noCertKey()) {
                continue;
            }

            Certificate[] certificates = new Certificate[entry.getValue().certificates().size()];
            int count = 0;
            for (X509Certificate x509Certificate : entry.getValue().certificates()) {
                certificates[count] = x509Certificate;
                count++;
            }

            // Alias will be FQDN
            keyStore.setKeyEntry(entry.getKey(), entry.getValue().privateKey(), password.toCharArray(), certificates);
        }

        FileOutputStream outputStream = new FileOutputStream(Profile.ensure(profile, true) + "_Server.pfx");
        keyStore.store(outputStream, password.toCharArray());
        outputStream.close();
    }

    /**
     * Load server certificates and private keys from PKCS12 password-protected store.
     *
     * @param tlsConfiguration {@link TLSConfiguration} to initialize certificates and private keys.
     * @param profile          Profile of this configuration.
     * @param password         Password to use for decryption PKCS12 store.
     * @throws Exception If any error occurs
     */
    static void loadServer(TLSConfiguration tlsConfiguration, String profile, String password) throws Exception {
        KeyStore keyStore = KeyStore.getInstance("PKCS12", new BouncyCastleProvider());
        FileInputStream inputStream = new FileInputStream(Profile.ensure(profile, true) + "_Server.pfx");
        keyStore.load(inputStream, password.toCharArray());

        Iterator<String> iterator = keyStore.aliases().asIterator();

        while (iterator.hasNext()) {
            String key = iterator.next();

            List<X509Certificate> x509Certificates = new ArrayList<>();
            Certificate[] certificates = keyStore.getCertificateChain(key);
            for (Certificate x509Certificate : certificates) {
                x509Certificates.add((X509Certificate) x509Certificate);
            }

            tlsConfiguration.addMapping(key, new CertificateKeyPair(x509Certificates, (PrivateKey) keyStore.getKey(key, password.toCharArray())));
        }

        inputStream.close();
    }

    /**
     * Save client certificate and private key into PKCS12 password-protected store.
     *
     * @param tlsConfiguration {@link TLSConfiguration} containing certificate and private key.
     * @param profile          Profile of this configuration.
     * @param password         Password to use for encryption PKCS12 store.
     * @throws Exception If any error occurs
     */
    static void saveClient(TLSConfiguration tlsConfiguration, String profile, String password) throws Exception {
        KeyStore keyStore = KeyStore.getInstance("PKCS12", new BouncyCastleProvider());
        keyStore.load(null, null);

        if (!tlsConfiguration.defaultMapping().noCertKey()) {
            Certificate[] certificates = new Certificate[]{(X509Certificate) tlsConfiguration.defaultMapping().certificates()};
            keyStore.setKeyEntry("DEFAULT_HOST", tlsConfiguration.defaultMapping().privateKey(), password.toCharArray(), certificates);
        }

        FileOutputStream outputStream = new FileOutputStream(Profile.ensure(profile, true) + "_Client.pfx");
        keyStore.store(outputStream, password.toCharArray());
        outputStream.close();
    }

    /**
     * Load client certificate and private key from PKCS12 password-protected store.
     *
     * @param tlsConfiguration {@link TLSConfiguration} to initialize client certificates and private key.
     * @param profile          Profile of this configuration.
     * @param password         Password to use for decryption PKCS12 store.
     * @throws Exception If any error occurs
     */
    static void loadClient(TLSConfiguration tlsConfiguration, String profile, String password) throws Exception {
        KeyStore keyStore = KeyStore.getInstance("PKCS12", new BouncyCastleProvider());
        FileInputStream inputStream = new FileInputStream(Profile.ensure(profile, true) + "_Client.pfx");
        keyStore.load(inputStream, password.toCharArray());

        List<X509Certificate> x509Certificates = new ArrayList<>();
        Certificate[] certificates = keyStore.getCertificateChain("DEFAULT_HOST");
        for (Certificate x509Certificate : certificates) {
            x509Certificates.add((X509Certificate) x509Certificate);
        }

        tlsConfiguration.defaultMapping(new CertificateKeyPair(x509Certificates, (PrivateKey) keyStore.getKey("DEFAULT_HOST", password.toCharArray())));

        inputStream.close();
    }
}
