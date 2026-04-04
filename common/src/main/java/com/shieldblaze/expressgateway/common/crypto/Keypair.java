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
import org.bouncycastle.asn1.pkcs.PrivateKeyInfo;
import org.bouncycastle.jce.provider.BouncyCastleProvider;
import org.bouncycastle.openssl.PEMKeyPair;
import org.bouncycastle.openssl.PEMParser;
import org.bouncycastle.openssl.jcajce.JcaPEMKeyConverter;

import java.io.IOException;
import java.io.StringReader;
import java.security.PrivateKey;
import java.security.Security;

/**
 * Parses PEM-encoded private keys using BouncyCastle.
 * Supports PKCS#1 (RSA/EC key pairs) and PKCS#8 (standalone private keys).
 */
@NoArgsConstructor(access = AccessLevel.PRIVATE)
public final class Keypair {

    private static final JcaPEMKeyConverter CONVERTER;

    static {
        Security.addProvider(new BouncyCastleProvider());
        CONVERTER = new JcaPEMKeyConverter().setProvider("BC");
    }

    public static PrivateKey parse(String privateKeyString) throws IOException {
        try (PEMParser pemParser = new PEMParser(new StringReader(privateKeyString))) {
            Object object = pemParser.readObject();

            PrivateKey key;
            if (object instanceof PEMKeyPair pemKeyPair) {
                key = CONVERTER.getKeyPair(pemKeyPair).getPrivate();
            } else if (object instanceof PrivateKeyInfo privateKeyInfo) {
                key = CONVERTER.getPrivateKey(privateKeyInfo);
            } else {
                throw new IllegalArgumentException("Unknown Private Key type: " +
                        (object != null ? object.getClass().getSimpleName() : "null"));
            }

            if ("DSA".equalsIgnoreCase(key.getAlgorithm())) {
                throw new IllegalArgumentException("DSA keys are not supported; use RSA or EC keys");
            }
            return key;
        }
    }
}
