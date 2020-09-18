/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.core.configuration.tls;

import io.netty.handler.ssl.OpenSsl;
import io.netty.util.internal.ObjectUtil;

import java.security.PrivateKey;
import java.security.cert.X509Certificate;
import java.util.List;

public final class ServerCertificateKey {
    private final PrivateKey privateKey;
    private final List<X509Certificate> certificateChain;

    public ServerCertificateKey(PrivateKey privateKey, List<X509Certificate> certificateChain, boolean ocspStapling) {
        this.privateKey = ObjectUtil.checkNotNull(privateKey, "Private Key");
        this.certificateChain = ObjectUtil.checkNonEmpty(certificateChain, "Certificate Chain");

        if (ocspStapling && !OpenSsl.isOcspSupported()) {
            throw new IllegalArgumentException("OCSP Stapling is not available because OpenSsl is not loaded.");
        }
    }

    PrivateKey getPrivateKey() {
        return privateKey;
    }

    List<X509Certificate> getCertificateChain() {
        return certificateChain;
    }
}
