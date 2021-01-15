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
package com.shieldblaze.expressgateway.controlinterface.configuration;

import com.google.protobuf.ProtocolStringList;
import com.shieldblaze.expressgateway.configuration.tls.CertificateKeyPair;
import com.shieldblaze.expressgateway.configuration.tls.Cipher;
import com.shieldblaze.expressgateway.configuration.tls.MutualTLS;
import com.shieldblaze.expressgateway.configuration.tls.Protocol;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfigurationBuilder;
import com.shieldblaze.expressgateway.controlinterface.tls.TLS;
import com.shieldblaze.expressgateway.controlinterface.tls.TLSServerServiceGrpc;
import io.grpc.stub.StreamObserver;

import java.io.FileWriter;
import java.io.IOException;
import java.security.cert.X509Certificate;
import java.util.ArrayList;
import java.util.Base64;
import java.util.HashMap;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.stream.Collectors;

public final class TLSServerService extends TLSServerServiceGrpc.TLSServerServiceImplBase {

    @Override
    public void server(TLS.Server request, StreamObserver<TLS.ConfigurationResponse> responseObserver) {
        TLS.ConfigurationResponse response;

        try {
            TLS.MutualTLS mTLS = request.getMTLS();
            MutualTLS mutualTLS;

            if (mTLS == TLS.MutualTLS.NOT_REQUIRED) {
                mutualTLS = MutualTLS.NOT_REQUIRED;
            } else if (mTLS == TLS.MutualTLS.OPTIONAL) {
                mutualTLS = MutualTLS.OPTIONAL;
            } else if (mTLS == TLS.MutualTLS.REQUIRED) {
                mutualTLS = MutualTLS.REQUIRED;
            } else {
                throw new IllegalArgumentException("Unknown MutualTLS Type: " + mTLS);
            }

            TLSConfiguration tlsConfiguration = TLSConfigurationBuilder.forServer()
                    .withUseALPN(request.getUseALPN())
                    .withUseStartTLS(request.getUseStartTLS())
                    .withSessionCacheSize(request.getSessionCacheSize())
                    .withSessionTimeout(request.getSessionTimeout())
                    .withProtocols(Utils.protocolConverter(request.getProtocolsList()))
                    .withCiphers(Utils.cipherConverter(request.getCiphersList()))
                    .withMutualTLS(mutualTLS)
                    .build();

            for (Map.Entry<String, TLS.Server.CertificateKeyPair> entry : request.getTlsServerMappingMap().entrySet()) {
                tlsConfiguration.addMapping(entry.getKey(), certificateKeyPairConverter(entry.getValue()));
            }

            tlsConfiguration.saveTo(request.getProfileName(), request.getPassword());

            response = TLS.ConfigurationResponse.newBuilder()
                    .setResponseCode(1)
                    .setResponseText("Success")
                    .build();

        } catch (Exception ex) {
            response = TLS.ConfigurationResponse.newBuilder()
                    .setResponseCode(-1)
                    .setResponseText("Error: " + ex.getLocalizedMessage())
                    .build();
        }

        responseObserver.onNext(response);
        responseObserver.onCompleted();
    }

    @Override
    public void get(TLS.GetTLS request, StreamObserver<TLS.Server> responseObserver) {
        try {
            TLSConfiguration tlsConfiguration = TLSConfiguration.loadFrom(request.getProfileName(), request.getPassword(), true);
            Map<String, TLS.Server.CertificateKeyPair> tlsMapping = new HashMap<>();

            for (Map.Entry<String, CertificateKeyPair> entry : tlsConfiguration.certificateKeyPairMap().entrySet()) {
                StringBuilder certificateChain = new StringBuilder();
                for (X509Certificate x509Certificate : entry.getValue().certificates()) {
                    certificateChain.append("-----BEGIN CERTIFICATE-----").append("\r\n");
                    certificateChain.append(Base64.getEncoder().encodeToString(x509Certificate.getEncoded())).append("\r\n");
                    certificateChain.append("-----END CERTIFICATE-----").append("\r\n");
                }

                tlsMapping.put(entry.getKey(),
                        TLS.Server.CertificateKeyPair.newBuilder()
                                .setCertificateChain(certificateChain.toString())
                                .setPrivateKey("")
                                .setUseOCSP(entry.getValue().useOCSPStapling())
                        .build());
            }

            TLS.MutualTLS mutualTLS = TLS.MutualTLS.NOT_REQUIRED;
            if (tlsConfiguration.mutualTLS() == MutualTLS.REQUIRED) {
                mutualTLS = TLS.MutualTLS.REQUIRED;
            } else if (tlsConfiguration.mutualTLS() == MutualTLS.OPTIONAL)  {
                mutualTLS = TLS.MutualTLS.OPTIONAL;
            }

            TLS.Server server = TLS.Server.newBuilder()
                    .putAllTlsServerMapping(tlsMapping)
                    .setMTLS(mutualTLS)
                    .setSessionTimeout(tlsConfiguration.sessionTimeout())
                    .setSessionCacheSize(tlsConfiguration.sessionCacheSize())
                    .addAllProtocols(tlsConfiguration.protocols().stream().map(Enum::name).collect(Collectors.toList()))
                    .addAllCiphers(tlsConfiguration.ciphers().stream().map(Enum::name).collect(Collectors.toList()))
                    .setUseStartTLS(tlsConfiguration.useStartTLS())
                    .setUseALPN(tlsConfiguration.useALPN())
                    .setProfileName(request.getProfileName())
                    .build();

            responseObserver.onNext(server);
            responseObserver.onCompleted();
        } catch (Exception ex) {
            responseObserver.onError(ex);
            responseObserver.onCompleted();
        }
    }

    private CertificateKeyPair certificateKeyPairConverter(TLS.Server.CertificateKeyPair certificateKeyPair) throws IOException {
        return new CertificateKeyPair(certificateKeyPair.getCertificateChain(), certificateKeyPair.getPrivateKey(), certificateKeyPair.getUseOCSP());
    }
}
