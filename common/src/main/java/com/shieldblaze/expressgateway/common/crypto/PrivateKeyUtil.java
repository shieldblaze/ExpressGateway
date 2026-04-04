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
package com.shieldblaze.expressgateway.common.crypto;

import lombok.AccessLevel;
import lombok.NoArgsConstructor;
import lombok.extern.log4j.Log4j2;
import org.bouncycastle.asn1.pkcs.PrivateKeyInfo;
import org.bouncycastle.jce.provider.BouncyCastleProvider;
import org.bouncycastle.openssl.PEMKeyPair;
import org.bouncycastle.openssl.PEMParser;
import org.bouncycastle.openssl.jcajce.JcaPEMKeyConverter;

import java.io.StringReader;
import java.security.KeyPair;
import java.security.PrivateKey;
import java.security.Security;

/**
 * Utility for parsing PEM-encoded private keys.
 *
 * <p>Supports PKCS#1 (RSA PRIVATE KEY), PKCS#8 (PRIVATE KEY), and
 * EC PRIVATE KEY formats via BouncyCastle PEM parsing. DSA keys are
 * explicitly rejected as they are cryptographically weak for TLS.</p>
 */
@Log4j2
@NoArgsConstructor(access = AccessLevel.PRIVATE)
public final class PrivateKeyUtil {

    private static final JcaPEMKeyConverter CONVERTER;

    static {
        Security.addProvider(new BouncyCastleProvider());
        CONVERTER = new JcaPEMKeyConverter().setProvider("BC");
    }

    /**
     * Parse a PEM-encoded private key string into a {@link PrivateKey}.
     *
     * @param pem PEM-encoded private key (PKCS#1, PKCS#8, or EC)
     * @return the parsed {@link PrivateKey}
     * @throws IllegalArgumentException if the key is invalid, unsupported, or DSA
     */
    public static PrivateKey parsePrivateKey(String pem) {
        try (PEMParser reader = new PEMParser(new StringReader(pem))) {
            Object obj = reader.readObject();

            if (obj instanceof PEMKeyPair pemKeyPair) {
                // PKCS#1 RSA or EC key pair
                KeyPair keyPair = CONVERTER.getKeyPair(pemKeyPair);
                rejectDSA(keyPair.getPrivate());
                return keyPair.getPrivate();
            }

            if (obj instanceof PrivateKeyInfo privateKeyInfo) {
                // PKCS#8 encoded private key
                PrivateKey privateKey = CONVERTER.getPrivateKey(privateKeyInfo);
                rejectDSA(privateKey);
                return privateKey;
            }

            throw new IllegalArgumentException("Unsupported PEM object type: " +
                    (obj != null ? obj.getClass().getSimpleName() : "null"));
        } catch (IllegalArgumentException ex) {
            throw ex;
        } catch (Exception ex) {
            log.warn("Failed to parse private key", ex);
            throw new IllegalArgumentException("Invalid Private Key", ex);
        }
    }

    private static void rejectDSA(PrivateKey key) {
        if ("DSA".equalsIgnoreCase(key.getAlgorithm())) {
            throw new IllegalArgumentException("DSA keys are not supported; use RSA or EC keys");
        }
    }
}
