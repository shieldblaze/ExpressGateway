package com.shieldblaze.expressgateway.common;

import jakarta.validation.constraints.NotNull;
import jakarta.validation.constraints.Size;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

public class JakartaValidatorTest {

    record TestObject(@NotNull(message = "Name cannot be null") @Size(min = 2, max = 14, message = "Name must be between 2 and 14 characters") String name) {
        TestObject(String name) {
            this.name = name;
        }
    }

    @Test
    void testValidObject() {
        TestObject validObject = new TestObject("ValidName");
        assertDoesNotThrow(() -> JakartaValidator.validate(validObject));
    }

    @Test
    void testNullName() {
        TestObject invalidObject = new TestObject(null);
        IllegalArgumentException exception = assertThrows(IllegalArgumentException.class, () -> JakartaValidator.validate(invalidObject));
        assertTrue(exception.getMessage().contains("Name cannot be null"));
    }

    @Test
    void testShortName() {
        TestObject invalidObject = new TestObject("A");
        IllegalArgumentException exception = assertThrows(IllegalArgumentException.class, () -> JakartaValidator.validate(invalidObject));
        assertTrue(exception.getMessage().contains("Name must be between 2 and 14 characters"));
    }

    @Test
    void testLongName() {
        TestObject invalidObject = new TestObject("ThisNameIsWayTooLong");
        IllegalArgumentException exception = assertThrows(IllegalArgumentException.class, () -> JakartaValidator.validate(invalidObject));
        assertTrue(exception.getMessage().contains("Name must be between 2 and 14 characters"));
    }
}
