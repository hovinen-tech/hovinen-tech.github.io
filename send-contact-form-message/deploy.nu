#!/usr/bin/env nu

let files = ["send-error.html"]

def main [contact_email: string, contact_phone: string] {
    for file in $files {
        sed -e $"s/{email}/($contact_email)/g" -e $"s/{phone}/($contact_phone)/g" $"backend-lambda/assets/($file).tmpl" | save $"backend-lambda/assets/($file)"
    }

    cargo lambda build -p backend-lambda --arm64
    cargo lambda deploy -p backend-lambda send-contact-form-message
}
