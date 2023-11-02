let scrollToTopButton = document.getElementById("back-to-top-button");

window.onscroll = () => {
    if (isBelowTop()) {
        scrollToTopButton.style.display = "block";
    } else {
        scrollToTopButton.style.display = "none";
    }
};

function isBelowTop() {
    return document.body.scrollTop > 20 ||
        document.documentElement.scrollTop > 20;
}

scrollToTopButton.addEventListener("click", () => {
    document.body.scrollTop = 0;
    document.documentElement.scrollTop = 0;
});
