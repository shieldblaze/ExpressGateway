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

import com.shieldblaze.expressgateway.configuration.Transformer;
import com.shieldblaze.expressgateway.configuration.BufferConfiguration;
import com.shieldblaze.expressgateway.restapi.response.FastBuilder;
import com.shieldblaze.expressgateway.restapi.response.builder.ErrorBase;
import com.shieldblaze.expressgateway.restapi.response.builder.Message;
import io.netty.handler.codec.http.HttpResponseStatus;
import io.swagger.v3.oas.annotations.Operation;
import io.swagger.v3.oas.annotations.Parameter;
import io.swagger.v3.oas.annotations.tags.Tag;
import org.springframework.http.HttpStatus;
import org.springframework.http.MediaType;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PathVariable;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

@RestController
@RequestMapping("/{profile}/config")
@Tag(name = "Buffer Configuration", description = "Create or Fetch Buffer Configuration")
public class BufferHandler {

    @Operation(summary = "Create or Modify Buffer Configuration", description = "Buffer configuration contains settings for ByteBuf." +
            " It is used to handle incoming or outgoing traffic.")
    @PostMapping(value = "/buffer", consumes = MediaType.APPLICATION_JSON_VALUE, produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> create(@Parameter(description = "Profile name")
                                         @PathVariable String profile,
                                         @Parameter(description = "JSON Body containing Buffer configuration")
                                         @RequestBody BufferConfiguration bufferConfiguration) {
        try {
            bufferConfiguration.validate();
            Transformer.write(bufferConfiguration, profile);
            return new ResponseEntity<>(HttpStatus.NO_CONTENT);
        } catch (Exception ex) {
            return FastBuilder.error(ErrorBase.REQUEST_ERROR, Message.newBuilder()
                    .withHeader("Error")
                    .withMessage(ex.getLocalizedMessage())
                    .build(), HttpResponseStatus.BAD_REQUEST);
        }
    }

    @Operation(summary = "Get the Buffer Configuration", description = "Get the Buffer configuration which is saved in the file.")
    @GetMapping(value = "/buffer", produces = MediaType.APPLICATION_JSON_VALUE)
    public ResponseEntity<String> get(@Parameter(description = "Profile name")
                                      @PathVariable String profile) {
        try {
            String data = Transformer.readJSON(BufferConfiguration.EMPTY_INSTANCE, profile);
            return new ResponseEntity<>(data, HttpStatus.OK);
        } catch (Exception ex) {
            return FastBuilder.error(ErrorBase.REQUEST_ERROR, Message.newBuilder()
                    .withHeader("Error")
                    .withMessage(ex.getLocalizedMessage())
                    .build(), HttpResponseStatus.BAD_REQUEST);
        }
    }
}
