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

import org.bouncycastle.jce.provider.BouncyCastleProvider;
import org.bouncycastle.openssl.PEMKeyPair;
import org.bouncycastle.openssl.PEMParser;
import org.bouncycastle.openssl.jcajce.JcaPEMKeyConverter;

import java.io.StringReader;
import java.security.KeyPair;
import java.security.PrivateKey;

public final class PrivateKeyUtil {

    private PrivateKeyUtil() {
        // Prevent outside initialization
    }

    public static PrivateKey parsePrivateKey(String pem) {
        try {
            PEMParser reader = new PEMParser(new StringReader(pem));

            Object obj = reader.readObject();
            if (obj instanceof PEMKeyPair pemKeyPair) {
                JcaPEMKeyConverter jcaPEMKeyConverter = new JcaPEMKeyConverter().setProvider(new BouncyCastleProvider());
                KeyPair keyPair = jcaPEMKeyConverter.getKeyPair(pemKeyPair);

                if ("DSA".equalsIgnoreCase(keyPair.getPrivate().getAlgorithm())) {
                    throw new IllegalArgumentException("Unsupported Private Key");
                }

                return keyPair.getPrivate();
            }
        } catch (IllegalArgumentException ex) {
            throw ex;
        } catch (Exception ex) {
            // Ignore
        }

        throw new IllegalArgumentException("Invalid Private Key");
    }
}
