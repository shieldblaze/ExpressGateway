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

import io.netty.handler.ssl.SslContext;
import io.netty.util.internal.ObjectUtil;
import org.bouncycastle.cert.ocsp.BasicOCSPResp;
import org.bouncycastle.cert.ocsp.OCSPResp;
import org.bouncycastle.cert.ocsp.SingleResp;

import java.io.IOException;
import java.security.PrivateKey;
import java.security.cert.X509Certificate;
import java.util.List;
import java.util.concurrent.TimeUnit;

/**
 * {@link X509Certificate} and {@link PrivateKey} Pair
 */
public final class CertificateKeyPair {
    private final List<X509Certificate> certificateChain;
    private final PrivateKey privateKey;
    private byte[] ocspStaplingData = null;
    private SingleResp ocspResp;
    private final boolean useOCSP;
    private SslContext sslContext;

    /**
     * Create new Instance of {@link CertificateKeyPair}.
     *
     * @param certificateChain {@link List} of {@link X509Certificate} Chain
     *                         <ol>
     *                            <li> First {@link X509Certificate} should be end-entity. </li>
     *                            <li> All {@link X509Certificate} in middle should be Intermediate Certificate Authority. </li>
     *                            <li> Last {@link X509Certificate} should be Root Certificate Authority. </li>
     *                         </ol>
     * @param privateKey       {@link PrivateKey} of our {@link X509Certificate}.
     * @param useOCSP          Set to {@code true} if we want OCSP Stapling (in case of Server) or
     *                         OCSP Validation (in case of Client) else set to {@code false}
     */
    public CertificateKeyPair(List<X509Certificate> certificateChain, PrivateKey privateKey, boolean useOCSP) {
        this.certificateChain = ObjectUtil.checkNonEmpty(certificateChain, "Certificate Chain");
        this.privateKey = ObjectUtil.checkNotNull(privateKey, "Private Key");
        this.useOCSP = useOCSP;
    }

    List<X509Certificate> getCertificateChain() {
        return certificateChain;
    }

    PrivateKey getPrivateKey() {
        return privateKey;
    }

    public boolean doOCSPCheck(boolean forServer) {
        if (useOCSP) {
            try {
                OCSPResp response =  OCSPClient.getResponse(certificateChain.get(0), certificateChain.get(1));
                if (forServer) {
                    ocspStaplingData = response.getEncoded();
                    ocspResp = ((BasicOCSPResp) response.getResponseObject()).getResponses()[0];
                    if (ocspResp.getCertStatus() == null) {
                        return true;
                    }
                } else {
                    if (((BasicOCSPResp) OCSPClient.getResponse(certificateChain.get(0), certificateChain.get(1)).getResponseObject())
                            .getResponses()[0].getCertStatus() == null) {
                        return true;
                    }
                }
            } catch (Exception ex) {
                // Ignore
            }
        }
        return false;
    }

    public byte[] getOcspStaplingData() throws IOException {
        if (ocspStaplingData != null) {
            long duration  = System.currentTimeMillis() - ocspResp.getNextUpdate().getTime();
            long diffInMinutes = TimeUnit.MILLISECONDS.toMinutes(duration);
            if (diffInMinutes > -60) {
                if (!doOCSPCheck(true)) {
                    throw new IOException("Unable To Renew OCSP Stapling Data");
                }
            }
        }
        return ocspStaplingData;
    }

    public SslContext getSslContext() {
        return sslContext;
    }

    void setSslContext(SslContext sslContext) {
        this.sslContext = sslContext;
    }

    public boolean useOCSP() {
        return useOCSP;
    }
}
