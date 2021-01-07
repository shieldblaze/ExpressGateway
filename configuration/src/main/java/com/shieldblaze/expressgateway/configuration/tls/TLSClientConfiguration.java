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

package com.shieldblaze.expressgateway.configuration.tls;

import com.fasterxml.jackson.annotation.JsonProperty;
import com.google.gson.annotations.Expose;
import com.shieldblaze.expressgateway.configuration.Configuration;
import io.netty.handler.ssl.ApplicationProtocolConfig;
import io.netty.handler.ssl.ApplicationProtocolNames;
import io.netty.handler.ssl.OpenSsl;
import io.netty.handler.ssl.SslContext;
import io.netty.handler.ssl.SslContextBuilder;
import io.netty.handler.ssl.SslProvider;
import io.netty.handler.ssl.util.InsecureTrustManagerFactory;

import javax.net.ssl.SSLException;
import javax.net.ssl.TrustManagerFactory;
import java.io.File;
import java.security.KeyStore;
import java.util.ArrayList;
import java.util.List;

public class TLSClientConfiguration implements Configuration {

    public static final TLSClientConfiguration EMPTY_INSTANCE = new TLSClientConfiguration();

    @Expose
    @JsonProperty("CertificateKeyPair")
    private CertificateKeyPair certificateKeyPair;

    @Expose
    @JsonProperty("ciphers")
    private List<Cipher> ciphers;

    @Expose
    @JsonProperty("protocols")
    private List<Protocol> protocols;

    @Expose
    @JsonProperty("acceptAllCerts")
    private boolean acceptAllCerts;

    @Expose
    @JsonProperty("useStartTLS")
    private boolean useStartTLS;

    private SslContext sslContext;

    public TLSClientConfiguration ciphers(List<Cipher> ciphers) {
        this.ciphers = ciphers;
        return this;
    }

    public TLSClientConfiguration protocols(List<Protocol> protocols) {
        this.protocols = protocols;
        return this;
    }

    public TLSClientConfiguration acceptAllCerts(boolean acceptAllCerts) {
        this.acceptAllCerts = acceptAllCerts;
        return this;
    }

    public TLSClientConfiguration useStartTLS(boolean useStartTLS) {
        this.useStartTLS = useStartTLS;
        return this;
    }

    public TLSClientConfiguration certificateKeyPair(CertificateKeyPair certificateKeyPair) {
        this.certificateKeyPair = certificateKeyPair;
        return this;
    }

    public SslContext sslContext() {
        return sslContext;
    }

    public void init() throws SSLException {
        TrustManagerFactory trustManagerFactory;
        if (acceptAllCerts) {
            trustManagerFactory = InsecureTrustManagerFactory.INSTANCE;
        } else {
            try {
                trustManagerFactory = TrustManagerFactory.getInstance(TrustManagerFactory.getDefaultAlgorithm());
                trustManagerFactory.init((KeyStore) null);
            } catch (Exception ex) {
                throw new IllegalArgumentException("Error occurred while building TrustManagerFactory", ex);
            }
        }

        List<String> _ciphers = new ArrayList<>();
        for (Cipher cipher : ciphers) {
            _ciphers.add(cipher.toString());
        }

        SslContextBuilder sslContextBuilder = SslContextBuilder.forClient()
                .sslProvider(OpenSsl.isAvailable() ? SslProvider.OPENSSL : SslProvider.JDK)
                .protocols(Protocol.getProtocols(protocols))
                .ciphers(_ciphers)
                .trustManager(trustManagerFactory)
                .startTls(useStartTLS)
                .applicationProtocolConfig(new ApplicationProtocolConfig(
                        ApplicationProtocolConfig.Protocol.ALPN,
                        ApplicationProtocolConfig.SelectorFailureBehavior.NO_ADVERTISE,
                        ApplicationProtocolConfig.SelectedListenerFailureBehavior.ACCEPT,
                        ApplicationProtocolNames.HTTP_2,
                        ApplicationProtocolNames.HTTP_1_1));

        if (certificateKeyPair != null) {
            sslContextBuilder.keyManager(new File(certificateKeyPair.certificateChain()).getAbsoluteFile(), new File(certificateKeyPair.privateKey()).getAbsoluteFile());
        }

        sslContext = sslContextBuilder.build();
    }

    @Override
    public String name() {
        return "TLSClient";
    }

    @Override
    public void validate() throws Exception {
        // Nothing to validate
    }
}
