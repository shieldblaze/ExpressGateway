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
package com.shieldblaze.expressgateway.configuration.transformer;

import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import com.shieldblaze.expressgateway.configuration.tls.CertificateKeyPair;
import com.shieldblaze.expressgateway.configuration.tls.Cipher;
import com.shieldblaze.expressgateway.configuration.tls.MutualTLS;
import com.shieldblaze.expressgateway.configuration.tls.Protocol;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.configuration.tls.TLSConfigurationBuilder;
import com.shieldblaze.expressgateway.configuration.tls.TLSServerMapping;

import java.io.File;
import java.io.FileWriter;
import java.io.IOException;
import java.nio.file.Files;
import java.util.ArrayList;
import java.util.List;
import java.util.Map;

public class TLS {

    private TLS() {
        // Prevent outside initialization
    }

    public static boolean write(TLSConfiguration configuration, String path) throws IOException {
        String jsonString = GSON.INSTANCE.toJson(configuration);

        try (FileWriter fileWriter = new FileWriter(path)) {
            fileWriter.write(jsonString);
        }

        return true;
    }

    public static TLSConfiguration readFile(String path) throws IOException {
        return readDirectly(Files.readString(new File(path).toPath()));
    }

    public static TLSConfiguration readDirectly(String data) throws IOException {
        JsonObject json = JsonParser.parseString(data).getAsJsonObject();

        boolean forServer = json.get("forServer").getAsBoolean();

        TLSConfigurationBuilder tlsConfigurationBuilder;
        if (forServer) {
            tlsConfigurationBuilder = TLSConfigurationBuilder.forServer();
        } else {
            tlsConfigurationBuilder = TLSConfigurationBuilder.forClient();
        }

        if (forServer) {
            JsonObject jsonObject = json.getAsJsonObject().get("certificateKeyPairMap").getAsJsonObject();

            TLSServerMapping tlsServerMapping = new TLSServerMapping();

            for (Map.Entry<String, JsonElement> entrySet : jsonObject.entrySet()) {
                JsonObject certAndKey = entrySet.getValue().getAsJsonObject();

                String certPath = certAndKey.get("certificateChain").getAsString();
                String keyPath = certAndKey.get("privateKey").getAsString();
                boolean ocsp = certAndKey.get("useOCSPStapling").getAsBoolean();

                CertificateKeyPair certificateKeyPair = new CertificateKeyPair(certPath, keyPath, ocsp);
                tlsServerMapping.mapping(entrySet.getKey(), certificateKeyPair);
            }

            tlsConfigurationBuilder.withTLSServerMapping(tlsServerMapping);
        } else {
            JsonObject certAndKey = json.get("certificateKeyPairMap").getAsJsonObject().get("DEFAULT_HOST").getAsJsonObject();
            String certPath = certAndKey.get("certificateChain").getAsString();
            String keyPath = certAndKey.get("privateKey").getAsString();
            boolean ocsp = certAndKey.get("useOCSPStapling").getAsBoolean();

            CertificateKeyPair certificateKeyPair = new CertificateKeyPair(certPath, keyPath, ocsp);
            tlsConfigurationBuilder.withClientCertificateKeyPair(certificateKeyPair);
        }

        boolean useStartTLS = json.get("useStartTLS").getAsBoolean();
        int sessionTimeout = json.get("sessionTimeout").getAsInt();
        int sessionCacheSize = json.get("sessionCacheSize").getAsInt();
        boolean acceptAllCerts = json.get("acceptAllCerts").getAsBoolean();

        List<Cipher> ciphers = new ArrayList<>();
        json.get("ciphers").getAsJsonArray().forEach(cipher -> ciphers.add(Cipher.valueOf(cipher.getAsString().toUpperCase())));

        List<Protocol> protocols = new ArrayList<>();
        json.get("protocols").getAsJsonArray().forEach(protocol -> protocols.add(Protocol.valueOf(protocol.getAsString().toUpperCase())));

        MutualTLS mutualTLS = MutualTLS.valueOf(json.get("mutualTLS").getAsString().toUpperCase());

        return tlsConfigurationBuilder
                .withCiphers(ciphers)
                .withProtocols(protocols)
                .withMutualTLS(mutualTLS)
                .withUseStartTLS(useStartTLS)
                .withSessionTimeout(sessionTimeout)
                .withSessionCacheSize(sessionCacheSize)
                .withAcceptAllCerts(acceptAllCerts)
                .build();
    }
}
