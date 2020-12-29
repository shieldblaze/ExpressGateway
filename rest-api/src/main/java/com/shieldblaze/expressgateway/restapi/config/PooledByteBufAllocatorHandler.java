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

import com.shieldblaze.expressgateway.configuration.buffer.PooledByteBufAllocatorConfiguration;
import com.shieldblaze.expressgateway.configuration.transformer.PooledByteBufAllocatorTransformer;
import org.springframework.http.HttpStatus;
import org.springframework.http.ResponseEntity;
import org.springframework.web.bind.annotation.GetMapping;
import org.springframework.web.bind.annotation.PostMapping;
import org.springframework.web.bind.annotation.RequestBody;
import org.springframework.web.bind.annotation.RequestMapping;
import org.springframework.web.bind.annotation.RestController;

import java.io.FileNotFoundException;
import java.nio.file.NoSuchFileException;

@RestController
@RequestMapping("/config")
public class PooledByteBufAllocatorHandler {

    @PostMapping("/buffer")
    public ResponseEntity<String> create(@RequestBody String data) {
        try {
            PooledByteBufAllocatorConfiguration pooledByteBufAllocatorConfiguration = PooledByteBufAllocatorTransformer.readDirectly(data);
            PooledByteBufAllocatorTransformer.write(pooledByteBufAllocatorConfiguration);
            return new ResponseEntity<>(HttpStatus.NO_CONTENT);
        } catch (FileNotFoundException | NoSuchFileException ex) {
            ex.printStackTrace();
            return new ResponseEntity<>("File not found", HttpStatus.NOT_FOUND);
        } catch (Exception ex) {
            return new ResponseEntity<>("Error Occurred: " + ex.getLocalizedMessage(), HttpStatus.BAD_REQUEST);
        }
    }

    @GetMapping("/buffer")
    public ResponseEntity<String> get() {
        try {
            String data = PooledByteBufAllocatorTransformer.getFileData();
            return new ResponseEntity<>(data, HttpStatus.OK);
        } catch (FileNotFoundException | NoSuchFileException ex) {
            return new ResponseEntity<>("File not found", HttpStatus.NOT_FOUND);
        } catch (Exception ex) {
            return new ResponseEntity<>("Error Occurred: " + ex.getLocalizedMessage(), HttpStatus.BAD_REQUEST);
        }
    }
}
