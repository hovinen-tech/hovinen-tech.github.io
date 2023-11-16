#!/usr/bin/env nu

let files = ["send-error.html", "send-error.de.html"]

def main [contact_email: string, contact_phone: string] {
    for file in $files {
        sed -e $"s/{email}/($contact_email)/g" -e $"s/{phone}/($contact_phone)/g" $"send-contact-form-message/assets/($file).tmpl" | save -f $"send-contact-form-message/assets/($file)"
    }

    cargo lambda build -p send-contact-form-message --arm64 --release
    cargo lambda deploy send-contact-form-message --region eu-north-1
}
