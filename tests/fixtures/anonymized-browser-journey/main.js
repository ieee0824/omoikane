import {answer} from './numbers.js';
journey.module = answer === 42;
const name = document.getElementById('name');
const status = document.getElementById('status');
document.getElementById('apply').addEventListener('click', () => {
    fetch('/api/data').then(response => response.json()).then(data => {
        if (data.answer !== answer) throw new Error('wrong answer');
        status.textContent = name.value + ': ' + data.answer;
        requestAnimationFrame(() => {
            status.style.backgroundColor = 'rgb(20,150,80)';
            journey.updated = true;
        });
    }).catch(error => journey.errors.push(String(error)));
});
document.getElementById('form').addEventListener('submit', () => {
    localStorage.setItem('journey-name', name.value);
});
