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

import java.io.ByteArrayInputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.security.cert.CertificateException;
import java.security.cert.CertificateFactory;
import java.security.cert.X509Certificate;

@NoArgsConstructor(access = AccessLevel.PRIVATE)
public final class CertificateUtil {

    private static final ThreadLocal<CertificateFactory> CERTIFICATE_FACTORY =
            ThreadLocal.withInitial(() -> {
                try {
                    return CertificateFactory.getInstance("X.509");
                } catch (CertificateException e) {
                    throw new IllegalStateException("X.509 CertificateFactory unavailable", e);
                }
            });

    public static X509Certificate parseX509Certificate(String certificate) {
        try (InputStream is = new ByteArrayInputStream(certificate.getBytes(StandardCharsets.UTF_8))) {
            return (X509Certificate) CERTIFICATE_FACTORY.get().generateCertificate(is);
        } catch (IOException | CertificateException e) {
            throw new IllegalArgumentException("Invalid Certificate", e);
        }
    }
}
