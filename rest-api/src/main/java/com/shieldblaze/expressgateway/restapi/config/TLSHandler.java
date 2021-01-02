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
package com.shieldblaze.expressgateway.restapi.config;

import com.shieldblaze.expressgateway.configuration.tls.TLSConfiguration;
import com.shieldblaze.expressgateway.configuration.transformer.TLSTransformer;
import com.shieldblaze.expressgateway.restapi.response.FastBuilder;
import com.shieldblaze.expressgateway.restapi.response.builder.ErrorBase;
import com.shieldblaze.expressgateway.restapi.response.builder.Message;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.swagger.v3.oas.annotations.Operation;
import io.swagger.v3.oas.annotations.tags.Tag;
import org.springframework.http.HttpStatus;
import org.springframework.http.MediaType;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
@RequestMapping("/config")
@Tag(name = "TLS Configuration", description = "Create or Fetch TLS Configuration")
public class TLSHandler {

    @Operation(summary = "Create or Modify TLS Configuration for Server",
            description = "TLS Configuration for Server contains settings for TLS in Server mode.")
    @PostMapping(value = "/tlsServer", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> createServer(@RequestBody String data) {
        try {
            TLSConfiguration tlsConfiguration = TLSTransformer.readDirectly(data);
            TLSTransformer.write(tlsConfiguration, true);
            return new ResponseEntity<>(HttpStatus.NO_CONTENT);
        } catch (Exception ex) {
            return FastBuilder.error(ErrorBase.REQUEST_ERROR, Message.newBuilder()
                    .withHeader("Error")
                    .withMessage(ex.getLocalizedMessage())
                    .build(), HttpResponseStatus.BAD_REQUEST);
        }
    }

    @Operation(summary = "Get the TLS for Server Configuration",
            description = "Get the TLS for Server configuration which is saved in the file.")
    @GetMapping(value = "/tlsServer", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getServer() {
        try {
            String data = TLSTransformer.getFileDataServer();
            return new ResponseEntity<>(data, HttpStatus.OK);
        } catch (Exception ex) {
            return FastBuilder.error(ErrorBase.REQUEST_ERROR, Message.newBuilder()
                    .withHeader("Error")
                    .withMessage(ex.getLocalizedMessage())
                    .build(), HttpResponseStatus.BAD_REQUEST);
        }
    }

    @Operation(summary = "Create or Modify TLS Configuration for Client",
            description = "TLS Configuration for Client contains settings for TLS in Client mode.")
    @PostMapping(value = "/tlsClient", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> createClient(@RequestBody String data) {
        try {
            TLSConfiguration tlsConfiguration = TLSTransformer.readDirectly(data);
            TLSTransformer.write(tlsConfiguration, false);
            return new ResponseEntity<>(HttpStatus.NO_CONTENT);
        } catch (Exception ex) {
            return FastBuilder.error(ErrorBase.REQUEST_ERROR, Message.newBuilder()
                    .withHeader("Error")
                    .withMessage(ex.getLocalizedMessage())
                    .build(), HttpResponseStatus.BAD_REQUEST);
        }
    }

    @Operation(summary = "Get the TLS for Client Configuration",
            description = "Get the TLS for Client configuration which is saved in the file.")
    @GetMapping(value = "/tlsClient", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> getClient() {
        try {
            String data = TLSTransformer.getFileDataClient();
            return new ResponseEntity<>(data, HttpStatus.OK);
        } catch (Exception ex) {
            return FastBuilder.error(ErrorBase.REQUEST_ERROR, Message.newBuilder()
                    .withHeader("Error")
                    .withMessage(ex.getLocalizedMessage())
                    .build(), HttpResponseStatus.BAD_REQUEST);
        }
    }
}
